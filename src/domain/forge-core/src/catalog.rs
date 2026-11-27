//! The capability catalog (#13): what each [`Action`] means, for people.
//!
//! The console's role editor is generated from this table. The `match` is
//! exhaustive, so a new action does not compile until it has an area, a
//! label, a description and a level here.

use serde::Serialize;

use crate::Action;

/// How much an action lets someone do, for the None / Read / Write / Admin
/// presets of the role editor. A preset selects every action of an area at
/// or below its level; it never adds hidden or future actions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AccessLevel {
    Read,
    Write,
    Admin,
}

/// One catalog row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ActionInfo {
    /// Area the action is grouped under in the role editor.
    pub area: &'static str,
    pub label: &'static str,
    pub description: &'static str,
    pub level: AccessLevel,
}

const fn info(area: &'static str, label: &'static str, description: &'static str, level: AccessLevel) -> ActionInfo {
    ActionInfo { area, label, description, level }
}

impl Action {
    /// The catalog row of this action.
    #[must_use]
    pub const fn info(self) -> ActionInfo {
        use AccessLevel::{Admin, Read, Write};

        match self {
            Action::View => {
                info("Fleet", "View", "See clusters, nodes, environments, services and jobs in scope.", Read)
            }
            Action::AuditRead => {
                info("Audit", "Read audit log", "Read the audit trail of everything done in scope.", Read)
            }
            Action::WorkspaceCreate => info("Platform", "Create workspaces", "Create new workspaces.", Admin),
            Action::WorkspaceManage => info(
                "Platform",
                "Manage workspace",
                "Change workspace settings, clusters, services and environments.",
                Admin,
            ),
            Action::UserManage => info(
                "Access control",
                "Manage members",
                "Create users, change group membership and assign roles in scope (only what you hold yourself).",
                Admin,
            ),
            Action::AccessManage => info(
                "Access control",
                "Manage roles",
                "Create, publish, duplicate and archive roles (access profiles) and groups.",
                Admin,
            ),
            Action::ServerEnroll => info("Nodes", "Enroll nodes", "Issue enrollment tokens and add nodes.", Admin),
            Action::NodeOnboard => info("Nodes", "Onboard nodes", "Install the agent on a node over SSH.", Admin),
            Action::ServerManage => {
                info("Nodes", "Manage nodes", "Maintenance, labels, diagnostics, revoke and remove nodes.", Write)
            }
            Action::ServerRename => info("Nodes", "Rename nodes", "Change a node's console name and DNS label.", Write),
            Action::NodeDrain => info(
                "Nodes",
                "Drain nodes",
                "Drain a node so placement skips it and its movable services move elsewhere; undrain it.",
                Write,
            ),
            Action::ServerCommand => {
                info("Nodes", "Run commands", "Run commands on a node as root (reboot, scripts).", Admin)
            }
            Action::ProcessManage => {
                info("Nodes", "Manage processes", "End processes and change their priority. Root-equivalent.", Admin)
            }
            Action::TerminalOpen => {
                info("Terminal", "Open terminal", "Open a shell on a node through its terminal account.", Admin)
            }
            Action::NetworkTunnel => info(
                "Network",
                "Open service tunnels",
                "Open port tunnels to the listed ports of services and packages.",
                Write,
            ),
            Action::NetworkTunnelAny => {
                info("Network", "Tunnel to any port", "Open a tunnel to any port of a node, listed or not.", Admin)
            }
            Action::ProxyManage => {
                info("Network", "Manage sites and DNS", "Configure reverse-proxy sites, routes and DNS records.", Admin)
            }
            Action::CertificateUse => {
                info("Network", "Use certificates", "Attach uploaded TLS certificates to sites.", Write)
            }
            Action::AlertView => info("Alerts", "View alerts", "See alarms and alert rules.", Read),
            Action::AlertManage => info(
                "Alerts",
                "Manage alerts",
                "Create and change alert rules; acknowledge alarms and add notes.",
                Write,
            ),
            Action::NotificationManage => info(
                "Alerts",
                "Manage notification channels",
                "Create, change and test where alarm messages are sent.",
                Admin,
            ),
            Action::HealthView => {
                info("Health", "View health checks", "Read health checks and their observations.", Read)
            }
            Action::HealthConfigure => {
                info("Health", "Configure health checks", "Change a service's HTTP and TCP probes.", Write)
            }
            Action::HealthCommand => {
                info("Health", "Command health checks", "Probes that run commands on a node.", Admin)
            }
            Action::LogRead => info("Logs", "Read logs", "Search and tail a service's logs.", Read),
            Action::LogManage => info(
                "Logs",
                "Manage log ingest keys",
                "Create and revoke the API keys a service uses to send its logs.",
                Write,
            ),
            Action::PackageManage => info(
                "Packages",
                "Manage packages",
                "Install, remove, start or stop App Store packages on a node.",
                Admin,
            ),
            Action::ContainerManage => info(
                "Containers",
                "Manage containers",
                "Start, stop, restart and remove containers; pull images.",
                Write,
            ),
            Action::ContainerInspectEnv => info(
                "Containers",
                "Read container environment",
                "See environment values on inspect. They often hold secrets.",
                Admin,
            ),
            Action::ContainerExec => {
                info("Containers", "Shell into containers", "Open an interactive shell inside a container.", Admin)
            }
            Action::StackManage => {
                info("Containers", "Manage stacks", "Create, update and remove compose stacks.", Write)
            }
            Action::StackPrivileged => info(
                "Containers",
                "Privileged stacks",
                "Deploy stacks with privileged settings (host network, mounts, capabilities).",
                Admin,
            ),
            Action::SwarmManage => info(
                "Containers",
                "Manage Docker Swarm",
                "Create and join swarms, and drain, promote, demote or remove swarm nodes.",
                Admin,
            ),
            Action::ImageManage => {
                info("Containers", "Manage images", "Pull, push, tag, remove and prune Docker images.", Write)
            }
            Action::VolumeManage => {
                info("Containers", "Manage volumes", "Create, remove and prune Docker volumes.", Write)
            }
            Action::NetworkManage => {
                info("Containers", "Manage networks", "Create, remove, connect and prune Docker networks.", Write)
            }
            Action::RegistryManage => info(
                "Containers",
                "Manage registries",
                "Add, change and remove container registries and their credentials.",
                Admin,
            ),
            Action::WebhookManage => info(
                "Services",
                "Manage deploy webhooks",
                "Create, rotate and remove webhooks that let CI deploy a service or stack.",
                Admin,
            ),
            Action::Deploy => info("Services", "Deploy", "Deploy and update services.", Write),
            Action::DeployApprove => info(
                "Services",
                "Approve deployments",
                "Approve or reject a deployment that waits for an approval.",
                Write,
            ),
            Action::JobCreate => info("Jobs", "Start jobs", "Start background jobs.", Write),
            Action::JobCancel => info("Jobs", "Cancel jobs", "Cancel running jobs.", Write),
            Action::FileRead => info("Files", "Read files", "List, read, search and download files.", Read),
            Action::FileWrite => {
                info("Files", "Write files", "Create, upload, edit, rename, move and copy files.", Write)
            }
            Action::FileDelete => info("Files", "Delete files", "Delete files and folders.", Write),
            Action::FileAdmin => info("Files", "File permissions", "Change ownership and permissions of files.", Admin),
            Action::FileNode => {
                info("Files", "Whole-node files", "Browse the whole node file system, not only approved roots.", Admin)
            }
            Action::StorageManage => info(
                "Object storage",
                "Manage object storage",
                "Create and remove stores and buckets; manage grants and keys.",
                Admin,
            ),
            Action::StorageData => info("Object storage", "Bucket data", "Read and write objects in buckets.", Write),
            Action::DbAdmin => {
                info("Databases", "Administer databases", "Configure databases, users, backups and restores.", Admin)
            }
            Action::SecretRead => info("Vault", "Read secrets", "Reveal secret values.", Read),
            Action::SecretManage => {
                info("Vault", "Manage secrets", "Create, change, rotate and delete secrets.", Admin)
            }
            Action::PluginInvoke => info("Plugins", "Use plugins", "Run plugin functions.", Write),
            Action::PluginManage => {
                info("Plugins", "Manage plugins", "Install, approve, configure and remove plugins.", Admin)
            }
            Action::SystemUpdate => info("Platform", "Update Forge", "Install Forge updates.", Admin),
            Action::AnalyticsConfigure => info(
                "Platform",
                "Configure analytics",
                "Change analytics retention, resource limits and search and ingest limits; restart the analytics engine.",
                Admin,
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_action_has_a_catalog_row() {
        for a in Action::ALL {
            let i = a.info();
            assert!(!i.area.is_empty() && !i.label.is_empty() && !i.description.is_empty(), "{a}");
        }
    }
}
