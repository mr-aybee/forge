//! Resolves the ancestry of any resource from the registry tables.

use async_trait::async_trait;
use forge_core::ports::{
    CertificateRepository, ClusterRepository, InstallationRepository, JobRepository, ProjectRepository,
    ResourceResolver, ServerRepository, ServiceRepository, TenancyRepository,
};
use forge_core::*;

use crate::Store;

fn parse_id<T: std::str::FromStr>(e: &EntityRef) -> DomainResult<T> {
    e.id.parse().map_err(|_| DomainError::Invalid(format!("bad id in {e}")))
}

impl Store {
    async fn workspace_chain(&self, org: WorkspaceId) -> DomainResult<Vec<EntityRef>> {
        // Validate existence so that a forged id cannot borrow a scope.
        self.get_workspace(org).await?;
        Ok(vec![EntityRef::new(EntityKind::Workspace, org)])
    }

    /// Environment → (project →) workspace. An environment's project is an
    /// authorization parent (D-102): a binding on the group covers its
    /// environments. Its cluster is placement, not a parent (D-056).
    async fn environment_chain(&self, id: EnvironmentId) -> DomainResult<Vec<EntityRef>> {
        let e = self.get_environment(id).await?;
        let mut chain = vec![EntityRef::new(EntityKind::Environment, e.id)];
        if let Some(g) = e.project_id {
            chain.push(EntityRef::new(EntityKind::Project, g));
        }
        chain.extend(self.workspace_chain(e.workspace_id).await?);
        Ok(chain)
    }

    async fn server_chain(&self, id: ServerId) -> DomainResult<Vec<EntityRef>> {
        let s = self.get_server(id).await?;
        let mut chain = vec![EntityRef::new(EntityKind::Server, s.id)];
        match s.environment_id {
            Some(env) => chain.extend(self.environment_chain(env).await?),
            None => chain.extend(self.workspace_chain(s.workspace_id).await?),
        }
        Ok(chain)
    }

    /// The entity itself followed by its ancestors.
    async fn chain(&self, e: &EntityRef) -> DomainResult<Vec<EntityRef>> {
        match e.kind {
            EntityKind::Platform | EntityKind::Plugin | EntityKind::User => Ok(vec![e.clone()]),
            EntityKind::Workspace => self.workspace_chain(parse_id(e)?).await,
            EntityKind::Tenant => {
                let t = self.get_tenant(parse_id(e)?).await?;
                Ok(vec![e.clone(), EntityRef::new(EntityKind::Workspace, t.workspace_id)])
            }
            EntityKind::Environment => self.environment_chain(parse_id(e)?).await,
            EntityKind::Server => self.server_chain(parse_id(e)?).await,
            EntityKind::Service => {
                let app = self
                    .get_service(parse_id(e)?)
                    .await?
                    .ok_or_else(|| DomainError::NotFound(format!("service {}", e.id)))?;
                let mut chain = vec![e.clone()];
                chain.extend(self.environment_chain(app.environment_id).await?);
                Ok(chain)
            }
            EntityKind::Cluster => {
                let c = self.get_cluster(parse_id(e)?).await?;
                Ok(vec![e.clone(), EntityRef::new(EntityKind::Workspace, c.workspace_id)])
            }
            EntityKind::Project => {
                let g = self.get_project(parse_id(e)?).await?;
                Ok(vec![e.clone(), EntityRef::new(EntityKind::Workspace, g.workspace_id)])
            }
            // Alarm resources (D-118): an installation sits on its node, an
            // ingress certificate in its workspace.
            EntityKind::Installation => {
                let i = self
                    .get_installation(parse_id(e)?)
                    .await?
                    .ok_or_else(|| DomainError::NotFound(format!("installation {}", e.id)))?;
                let mut chain = vec![e.clone()];
                chain.extend(self.server_chain(i.server_id).await?);
                Ok(chain)
            }
            EntityKind::Certificate => {
                let c = self
                    .get_certificate(parse_id(e)?)
                    .await?
                    .ok_or_else(|| DomainError::NotFound(format!("certificate {}", e.id)))?;
                Ok(vec![e.clone(), EntityRef::new(EntityKind::Workspace, c.workspace_id)])
            }
            EntityKind::AuditLog => {
                if e.id == "platform" {
                    Ok(vec![e.clone()])
                } else {
                    let mut chain = vec![e.clone()];
                    chain.extend(self.workspace_chain(parse_id(e)?).await?);
                    Ok(chain)
                }
            }
            EntityKind::Job => {
                let job = self.get_job(parse_id(e)?).await?;
                let mut chain = vec![e.clone()];
                if job.resource.kind != EntityKind::Job {
                    chain.extend(Box::pin(self.chain(&job.resource)).await?);
                }
                Ok(chain)
            }
        }
    }
}

#[async_trait]
impl ResourceResolver for Store {
    async fn resolve(&self, entity: &EntityRef) -> DomainResult<ResourceRef> {
        let mut chain = self.chain(entity).await?;
        let first = chain.remove(0);
        let servers: Vec<EntityRef> =
            std::iter::once(&first).chain(chain.iter()).filter(|e| e.kind == EntityKind::Server).cloned().collect();
        let mut resource = ResourceRef::new(first, chain);
        // A node in a cluster has a second parent: server → cluster → workspace.
        for server in servers {
            let s = self.get_server(parse_id(&server)?).await?;
            if let Some(cluster) = s.cluster_id {
                resource.add_chain(&[
                    server,
                    EntityRef::new(EntityKind::Cluster, cluster),
                    EntityRef::new(EntityKind::Workspace, s.workspace_id),
                ]);
            }
        }
        Ok(resource)
    }

    /// Servers in one pass: one query for the server rows, and each
    /// environment / workspace chain looked up once (list endpoints authorize
    /// every server; this turns ~4 queries per server into a handful).
    async fn resolve_many(&self, entities: &[EntityRef]) -> Vec<DomainResult<ResourceRef>> {
        if entities.len() < 2 || entities.iter().any(|e| e.kind != EntityKind::Server) {
            let mut out = Vec::with_capacity(entities.len());
            for e in entities {
                out.push(self.resolve(e).await);
            }
            return out;
        }
        let rows: std::collections::HashMap<String, forge_core::model::Server> = match self.list_servers(None).await {
            Ok(list) => list.into_iter().map(|s| (s.id.to_string(), s)).collect(),
            Err(e) => return entities.iter().map(|_| Err(DomainError::internal(&e))).collect(),
        };
        let mut ns_memo: std::collections::HashMap<EnvironmentId, Option<Vec<EntityRef>>> =
            std::collections::HashMap::new();
        let mut ws_memo: std::collections::HashMap<WorkspaceId, Option<Vec<EntityRef>>> =
            std::collections::HashMap::new();
        let mut out = Vec::with_capacity(entities.len());
        for e in entities {
            let Some(s) = rows.get(&e.id) else {
                out.push(Err(DomainError::NotFound(format!("server {}", e.id))));
                continue;
            };
            let parent = match s.environment_id {
                Some(ns) => match ns_memo.get(&ns) {
                    Some(c) => c.clone(),
                    None => {
                        let c = self.environment_chain(ns).await.ok();
                        ns_memo.insert(ns, c.clone());
                        c
                    }
                },
                None => match ws_memo.get(&s.workspace_id) {
                    Some(c) => c.clone(),
                    None => {
                        let c = self.workspace_chain(s.workspace_id).await.ok();
                        ws_memo.insert(s.workspace_id, c.clone());
                        c
                    }
                },
            };
            let Some(parent) = parent else {
                out.push(Err(DomainError::NotFound(format!("ancestry of server {}", e.id))));
                continue;
            };
            let server = EntityRef::new(EntityKind::Server, s.id);
            let mut resource = ResourceRef::new(server.clone(), parent);
            if let Some(cluster) = s.cluster_id {
                resource.add_chain(&[
                    server,
                    EntityRef::new(EntityKind::Cluster, cluster),
                    EntityRef::new(EntityKind::Workspace, s.workspace_id),
                ]);
            }
            out.push(Ok(resource));
        }
        out
    }
}
