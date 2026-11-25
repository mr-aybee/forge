use std::collections::{HashMap, HashSet};
use std::str::FromStr;
use std::sync::RwLock;

use cedar_policy::{
    Authorizer as CedarEngine, Context, Decision as CedarDecision, Entities, Entity, EntityId, EntityTypeName,
    EntityUid, PolicyId, PolicySet, Request, RestrictedExpression, Schema, SlotId, Template, ValidationMode, Validator,
};
use forge_core::model::RoleBinding;
use forge_core::permissions::{Assignee, ProfileAssignment};
use forge_core::{Action, AuthzContext, EntityKind, EntityRef, Principal, ResourceRef};
use forge_core::{GroupId, PermissionProfileId, ProfileAssignmentId, UserId};
use thiserror::Error;

use crate::{GUARDRAILS_SRC, ROLE_TEMPLATES_SRC, SCHEMA_SRC};

const NAMESPACE: &str = "Forge";

#[derive(Debug, Error)]
pub enum AuthzError {
    #[error("cedar schema error: {0}")]
    Schema(String),
    #[error("cedar policy error: {0}")]
    Policy(String),
    #[error("policy validation failed: {0}")]
    Validation(String),
    #[error("invalid request: {0}")]
    Request(String),
}

/// Result of evaluating one request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decision {
    pub allowed: bool,
    /// Ids of the policies that determined the decision.
    pub reasons: Vec<String>,
    /// Evaluation errors (treated as deny).
    pub errors: Vec<String>,
}

/// Owns policies and evaluates fully-resolved requests. Thread-safe; links
/// can be added and removed while requests are evaluated.
pub struct PolicyEngine {
    schema: Schema,
    policies: RwLock<PolicySet>,
    engine: CedarEngine,
    /// Expiry (epoch ms) of each linked assignment that has one. Every
    /// decision first unlinks those whose time has come, so an expired
    /// assignment never grants, whatever the sweeper does (D-086).
    expiries: std::sync::Mutex<std::collections::BTreeMap<(i64, String), ProfileAssignmentId>>,
    /// The time expiry is judged by (the server's clock).
    clock: std::sync::Arc<dyn forge_core::Clock>,
    /// The groups of each user (#13). Becomes the parents of the principal
    /// entity, so a link to a group (`principal in ?principal`) grants every
    /// member. Changing it takes effect at the next decision.
    memberships: RwLock<HashMap<UserId, HashSet<GroupId>>>,
}

impl std::fmt::Debug for PolicyEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PolicyEngine").finish_non_exhaustive()
    }
}

impl PolicyEngine {
    /// Loads the embedded schema, templates and guardrails and validates
    /// them strictly against the schema.
    pub fn new() -> Result<Self, AuthzError> {
        Self::with_clock(std::sync::Arc::new(forge_core::SystemClock))
    }

    /// [`Self::new`] with the clock assignment expiry is judged by.
    pub fn with_clock(clock: std::sync::Arc<dyn forge_core::Clock>) -> Result<Self, AuthzError> {
        let (schema, _warnings) =
            Schema::from_cedarschema_str(SCHEMA_SRC).map_err(|e| AuthzError::Schema(e.to_string()))?;

        let mut set = PolicySet::new();
        let templates = PolicySet::from_str(ROLE_TEMPLATES_SRC).map_err(|e| AuthzError::Policy(e.to_string()))?;

        for t in templates.templates() {
            let id = annotated_id(t.annotation("id"))?;
            set.add_template(t.new_id(id)).map_err(|e| AuthzError::Policy(e.to_string()))?;
        }

        let guards = PolicySet::from_str(GUARDRAILS_SRC).map_err(|e| AuthzError::Policy(e.to_string()))?;

        for p in guards.policies() {
            let id = annotated_id(p.annotation("id"))?;
            set.add(p.new_id(id)).map_err(|e| AuthzError::Policy(e.to_string()))?;
        }

        let result = Validator::new(schema.clone()).validate(&set, ValidationMode::Strict);
        if !result.validation_passed() {
            let msgs: Vec<String> = result.validation_errors().map(ToString::to_string).collect();
            return Err(AuthzError::Validation(msgs.join("; ")));
        }

        Ok(Self {
            schema,
            policies: RwLock::new(set),
            engine: CedarEngine::new(),
            expiries: std::sync::Mutex::new(std::collections::BTreeMap::new()),
            clock,
            memberships: RwLock::new(HashMap::new()),
        })
    }

    /// Links the role template for `binding` (one Cedar policy per binding).
    pub fn link_binding(&self, binding: &RoleBinding) -> Result<(), AuthzError> {
        let template = PolicyId::new(format!("role:{}", binding.role.as_str()));
        let link_id = PolicyId::new(binding.id.to_string());
        let vals = HashMap::from([
            (SlotId::principal(), principal_uid(&Principal::User(binding.user_id))?),
            (SlotId::resource(), entity_uid(&binding.scope)?),
        ]);
        let mut set = self.policies.write().map_err(|_| AuthzError::Policy("lock poisoned".into()))?;

        set.link(template, link_id, vals).map_err(|e| AuthzError::Policy(e.to_string()))
    }

    /// Replaces every group membership (start-up).
    pub fn set_memberships(&self, pairs: &[(GroupId, UserId)]) {
        let mut map: HashMap<UserId, HashSet<GroupId>> = HashMap::new();

        for (g, u) in pairs {
            map.entry(*u).or_default().insert(*g);
        }

        *self.memberships.write().unwrap_or_else(std::sync::PoisonError::into_inner) = map;
    }

    /// Makes `user` a member of `group`; idempotent.
    pub fn add_member(&self, group: GroupId, user: UserId) {
        self.memberships
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .entry(user)
            .or_default()
            .insert(group);
    }

    /// Removes `user` from `group`; idempotent. Takes effect at once.
    pub fn remove_member(&self, group: GroupId, user: UserId) {
        let mut m = self.memberships.write().unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(gs) = m.get_mut(&user) {
            gs.remove(&group);

            if gs.is_empty() {
                m.remove(&user);
            }
        }
    }

    /// Forgets a deleted group: no user is a member any more.
    pub fn drop_group(&self, group: GroupId) {
        let mut m = self.memberships.write().unwrap_or_else(std::sync::PoisonError::into_inner);

        for gs in m.values_mut() {
            gs.remove(&group);
        }

        m.retain(|_, gs| !gs.is_empty());
    }

    /// The groups `user` is a member of.
    #[must_use]
    pub fn groups_of(&self, user: UserId) -> Vec<GroupId> {
        self.memberships
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&user)
            .map(|g| g.iter().copied().collect())
            .unwrap_or_default()
    }

    /// The actions a built-in role grants, read from the schema's action
    /// groups (`action "x" in ["role:…"]`), so built-in profiles always match
    /// the roles exactly. Sorted by name.
    pub fn role_actions(&self, role: forge_core::Role) -> Result<Vec<Action>, AuthzError> {
        let entities = self.schema.action_entities().map_err(|e| AuthzError::Schema(e.to_string()))?;
        let group = action_group_uid(role.as_str())?;
        let mut out = Vec::new();

        for a in Action::ALL {
            let uid = action_uid(a)?;
            let in_group = entities.ancestors(&uid).is_some_and(|mut it| it.any(|g| *g == group));
            if in_group {
                out.push(a);
            }
        }

        out.sort_by_key(|a| a.as_str());

        Ok(out)
    }

    /// Template id of one profile revision.
    fn profile_template_id(profile: PermissionProfileId, revision: u32) -> PolicyId {
        PolicyId::new(format!("profile:{profile}:r{revision}"))
    }

    /// Link id of one assignment.
    fn assignment_link_id(id: ProfileAssignmentId) -> PolicyId {
        PolicyId::new(format!("assign:{id}"))
    }

    /// Adds the template for one profile revision (D-086): `permit` for the
    /// linked principal, these actions, and the linked scope and everything
    /// below it. Built as Cedar's JSON policy form from [`Action`] values
    /// only; no name, description or other user text reaches it. Adding a
    /// revision that is already there is a no-op.
    pub fn add_profile_revision(
        &self,
        profile: PermissionProfileId,
        revision: u32,
        actions: &[Action],
    ) -> Result<(), AuthzError> {
        if actions.is_empty() {
            return Err(AuthzError::Policy("a profile needs at least one action".into()));
        }

        let id = Self::profile_template_id(profile, revision);
        let entities: Vec<serde_json::Value> = actions
            .iter()
            .map(|a| serde_json::json!({ "type": format!("{NAMESPACE}::Action"), "id": a.as_str() }))
            .collect();
        let est = serde_json::json!({
            "effect": "permit",
            "principal": { "op": "in", "slot": "?principal" },
            "action": { "op": "in", "entities": entities },
            "resource": { "op": "in", "slot": "?resource" },
            "conditions": []
        });
        let template = Template::from_json(Some(id.clone()), est).map_err(|e| AuthzError::Policy(e.to_string()))?;
        let mut set = self.policies.write().map_err(|_| AuthzError::Policy("lock poisoned".into()))?;
        if set.template(&id).is_some() {
            return Ok(());
        }

        let mut probe = PolicySet::new();
        probe.add_template(template.clone()).map_err(|e| AuthzError::Policy(e.to_string()))?;
        let result = Validator::new(self.schema.clone()).validate(&probe, ValidationMode::Strict);
        if !result.validation_passed() {
            let msgs: Vec<String> = result.validation_errors().map(ToString::to_string).collect();
            return Err(AuthzError::Validation(msgs.join("; ")));
        }

        set.add_template(template).map_err(|e| AuthzError::Policy(e.to_string()))
    }

    /// Links `assignment` to its profile's revision `revision` (added with
    /// [`Self::add_profile_revision`]). Replaces an earlier link of the same
    /// assignment, so publishing a revision moves every assignment over.
    pub fn link_assignment(&self, assignment: &ProfileAssignment, revision: u32) -> Result<(), AuthzError> {
        let template = Self::profile_template_id(assignment.profile_id, revision);
        let link_id = Self::assignment_link_id(assignment.id);

        let who = match assignment.assignee() {
            Some(Assignee::User(u)) => principal_uid(&Principal::User(u))?,
            Some(Assignee::Group(g)) => group_uid(g)?,
            None => return Err(AuthzError::Policy("an assignment names exactly one user or group".into())),
        };

        let vals = HashMap::from([(SlotId::principal(), who), (SlotId::resource(), entity_uid(&assignment.scope)?)]);

        if assignment.expires_at.is_some_and(|e| e <= self.clock.now()) {
            return self.unlink_assignment(assignment.id);
        }

        {
            let mut set = self.policies.write().map_err(|_| AuthzError::Policy("lock poisoned".into()))?;
            let _ = set.unlink(link_id.clone());
            set.link(template, link_id, vals).map_err(|e| AuthzError::Policy(e.to_string()))?;
        }

        if let Some(at) = assignment.expires_at {
            let mut ex = self.expiries.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            ex.retain(|_, id| *id != assignment.id);
            ex.insert((at.0, assignment.id.to_string()), assignment.id);
        }

        Ok(())
    }

    /// Unlinks every assignment whose expiry is at or before `now`.
    fn drop_expired(&self, now: i64) {
        let due: Vec<ProfileAssignmentId> = {
            let mut ex = self.expiries.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            let Some((&(first, _), _)) = ex.first_key_value() else { return };
            if first > now {
                return;
            }

            let later = ex.split_off(&(now.saturating_add(1), String::new()));
            let due = std::mem::replace(&mut *ex, later);
            due.into_values().collect()
        };

        let mut set = self.policies.write().unwrap_or_else(std::sync::PoisonError::into_inner);

        for id in due {
            let _ = set.unlink(Self::assignment_link_id(id));
        }
    }

    /// Removes an assignment's link. Unknown ids are ignored (idempotent).
    pub fn unlink_assignment(&self, id: ProfileAssignmentId) -> Result<(), AuthzError> {
        self.expiries.lock().unwrap_or_else(std::sync::PoisonError::into_inner).retain(|_, x| *x != id);
        let mut set = self.policies.write().map_err(|_| AuthzError::Policy("lock poisoned".into()))?;
        let _ = set.unlink(Self::assignment_link_id(id));

        Ok(())
    }

    /// Removes a binding's link. Unknown ids are ignored (idempotent).
    pub fn unlink_binding(&self, binding_id: &str) -> Result<(), AuthzError> {
        let mut set = self.policies.write().map_err(|_| AuthzError::Policy("lock poisoned".into()))?;
        match set.unlink(PolicyId::new(binding_id)) {
            Ok(_) => Ok(()),
            Err(e) => {
                tracing::debug!(binding_id, error = %e, "unlink ignored");
                Ok(())
            }
        }
    }

    /// Evaluates one request. Any construction or evaluation error denies.
    pub fn evaluate(
        &self,
        principal: &Principal,
        action: Action,
        resource: &ResourceRef,
        ctx: AuthzContext,
    ) -> Result<Decision, AuthzError> {
        self.drop_expired(self.clock.now().0);
        let p_uid = principal_uid(principal)?;
        let a_uid = action_uid(action)?;
        let r_uid = entity_uid(&resource.entity)?;
        let entities = self.entities_for(principal, &p_uid, resource)?;
        let context = Context::from_pairs([("mfa".to_owned(), RestrictedExpression::new_bool(ctx.mfa))])
            .map_err(|e| AuthzError::Request(e.to_string()))?;
        let request = Request::new(p_uid, a_uid, r_uid, context, Some(&self.schema))
            .map_err(|e| AuthzError::Request(e.to_string()))?;

        let set = self.policies.read().map_err(|_| AuthzError::Policy("lock poisoned".into()))?;
        let response = self.engine.is_authorized(&request, &set, &entities);
        let errors: Vec<String> = response.diagnostics().errors().map(ToString::to_string).collect();

        Ok(Decision {
            allowed: response.decision() == CedarDecision::Allow && errors.is_empty(),
            reasons: response.diagnostics().reason().map(ToString::to_string).collect(),
            errors,
        })
    }

    /// Whether `principal` holds `action` over `resource` and everything
    /// beneath it (#13): the same policies and entities as
    /// [`Self::evaluate`], but the request is not checked against the
    /// schema, so the action need not apply to the resource's own type. A
    /// `permit … resource in ?scope` covering the resource matches; nothing
    /// else can. For grant checks only, never to authorize an operation.
    pub fn holds_over(
        &self,
        principal: &Principal,
        action: Action,
        resource: &ResourceRef,
        ctx: AuthzContext,
    ) -> Result<Decision, AuthzError> {
        self.drop_expired(self.clock.now().0);
        let p_uid = principal_uid(principal)?;
        let entities = self.entities_for(principal, &p_uid, resource)?;
        let context = Context::from_pairs([("mfa".to_owned(), RestrictedExpression::new_bool(ctx.mfa))])
            .map_err(|e| AuthzError::Request(e.to_string()))?;
        let request = Request::new(p_uid, action_uid(action)?, entity_uid(&resource.entity)?, context, None)
            .map_err(|e| AuthzError::Request(e.to_string()))?;
        let set = self.policies.read().map_err(|_| AuthzError::Policy("lock poisoned".into()))?;
        let response = self.engine.is_authorized(&request, &set, &entities);
        let errors: Vec<String> = response.diagnostics().errors().map(ToString::to_string).collect();

        Ok(Decision {
            allowed: response.decision() == CedarDecision::Allow && errors.is_empty(),
            reasons: response.diagnostics().reason().map(ToString::to_string).collect(),
            errors,
        })
    }

    /// Evaluates one principal and action against many resources with one
    /// entity set (the union of their graphs; each entity keeps its own
    /// parents, so every decision equals [`PolicyEngine::evaluate`]).
    pub fn evaluate_many(
        &self,
        principal: &Principal,
        action: Action,
        resources: &[ResourceRef],
        ctx: AuthzContext,
    ) -> Result<Vec<Decision>, AuthzError> {
        self.drop_expired(self.clock.now().0);
        let p_uid = principal_uid(principal)?;
        let a_uid = action_uid(action)?;
        let mut merged: Vec<(EntityRef, Vec<EntityRef>)> = Vec::new();
        let mut seen: std::collections::HashMap<EntityRef, usize> = std::collections::HashMap::new();

        for r in resources {
            for (node, parents) in &r.parents {
                match seen.get(node) {
                    Some(&i) => {
                        let ps: &mut Vec<EntityRef> = &mut merged[i].1;

                        for p in parents {
                            if !ps.contains(p) {
                                ps.push(p.clone());
                            }
                        }
                    }
                    None => {
                        seen.insert(node.clone(), merged.len());
                        merged.push((node.clone(), parents.clone()));
                    }
                }
            }
        }

        let graph = ResourceRef { entity: EntityRef::platform(), ancestors: vec![], parents: merged };
        let entities = self.entities_for(principal, &p_uid, &graph)?;
        let set = self.policies.read().map_err(|_| AuthzError::Policy("lock poisoned".into()))?;
        let mut out = Vec::with_capacity(resources.len());

        for r in resources {
            let context = Context::from_pairs([("mfa".to_owned(), RestrictedExpression::new_bool(ctx.mfa))])
                .map_err(|e| AuthzError::Request(e.to_string()))?;
            let request =
                Request::new(p_uid.clone(), a_uid.clone(), entity_uid(&r.entity)?, context, Some(&self.schema));

            let Ok(request) = request else {
                out.push(Decision {
                    allowed: false,
                    reasons: vec![],
                    errors: vec!["request does not fit the schema".into()],
                });
                continue;
            };

            let response = self.engine.is_authorized(&request, &set, &entities);
            let errors: Vec<String> = response.diagnostics().errors().map(ToString::to_string).collect();
            out.push(Decision {
                allowed: response.decision() == CedarDecision::Allow && errors.is_empty(),
                reasons: response.diagnostics().reason().map(ToString::to_string).collect(),
                errors,
            });
        }

        Ok(out)
    }

    /// Builds the entity slice for one request: the principal with its
    /// groups as parents, the groups, and every node of the resource graph
    /// with its parents. Parentless nodes other than the platform hang off
    /// the platform root.
    fn entities_for(
        &self,
        who: &Principal,
        principal: &EntityUid,
        resource: &ResourceRef,
    ) -> Result<Entities, AuthzError> {
        let platform = EntityRef::platform();

        let groups = match who {
            Principal::User(u) => self.groups_of(*u),
        };

        let mut entities = Vec::with_capacity(resource.parents.len() + groups.len() + 2);
        let group_uids = groups.into_iter().map(group_uid).collect::<Result<HashSet<_>, _>>()?;

        for g in &group_uids {
            entities.push(Entity::new_no_attrs(g.clone(), HashSet::new()));
        }

        entities.push(Entity::new_no_attrs(principal.clone(), group_uids));
        let mut has_platform = false;

        for (node, parents) in &resource.parents {
            has_platform |= node.kind == EntityKind::Platform;
            let mut ps = parents.iter().map(entity_uid).collect::<Result<HashSet<_>, _>>()?;
            if ps.is_empty() && node.kind != EntityKind::Platform {
                ps.insert(entity_uid(&platform)?);
            }

            entities.push(Entity::new_no_attrs(entity_uid(node)?, ps));
        }

        if !has_platform {
            entities.push(Entity::new_no_attrs(entity_uid(&platform)?, HashSet::new()));
        }

        Entities::from_entities(entities, Some(&self.schema)).map_err(|e| AuthzError::Request(e.to_string()))
    }
}

fn annotated_id(annotation: Option<&str>) -> Result<PolicyId, AuthzError> {
    annotation
        .map(PolicyId::new)
        .ok_or_else(|| AuthzError::Policy("every policy and template needs an @id annotation".into()))
}

fn uid(type_name: &str, id: &str) -> Result<EntityUid, AuthzError> {
    let t = EntityTypeName::from_str(&format!("{NAMESPACE}::{type_name}"))
        .map_err(|e| AuthzError::Request(e.to_string()))?;
    Ok(EntityUid::from_type_name_and_id(t, EntityId::new(id)))
}

fn entity_uid(e: &EntityRef) -> Result<EntityUid, AuthzError> {
    uid(e.kind.cedar_type(), &e.id)
}

fn principal_uid(p: &Principal) -> Result<EntityUid, AuthzError> {
    match p {
        Principal::User(id) => uid(EntityKind::User.cedar_type(), &id.to_string()),
    }
}

fn group_uid(g: GroupId) -> Result<EntityUid, AuthzError> {
    uid("Group", &g.to_string())
}

fn action_uid(a: Action) -> Result<EntityUid, AuthzError> {
    uid("Action", a.as_str())
}

fn action_group_uid(role: &str) -> Result<EntityUid, AuthzError> {
    uid("Action", &format!("role:{role}"))
}
