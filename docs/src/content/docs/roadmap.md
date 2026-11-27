---
title: Roadmap
description: What is planned next.
---

- **Integrations:** GitHub (deploy from a repository on push, webhooks, deploy
  keys), Google Cloud Storage upload/sync, S3 backups (databases and volumes,
  RustFS included), SMTP relay.
- **Services:** releases and artefact uploads, blue-green switching,
  health-check driven failover (DNS / standby promotion), session recording
  and per-role login restrictions for the browser terminal, SFTP upload.
- **Proxies:** Traefik and YARP as alternatives to Nginx, a WAF (Coraza),
  DNS-01 for wildcard certificates, certificate expiry alerts.
- **Platforms:** RHEL-family package recipes, Windows packages and
  .NET Framework hosting (IIS). Container orchestrators (Swarm, Kubernetes)
  are parked until there is a planned use (D-033).
- **Windows workload accounts:** run what Forge installs on a Windows node
  under an account you choose instead of SYSTEM: a per-service virtual
  account by default, Local Service, Network Service or a user from the
  credential vault, as a node default with a per-app override. Package services such as PostgreSQL
  first, then native apps on
  Windows (D-081).
- **Monitoring:** stored, long-term node metrics and service metrics
  with dashboards. Live CPU, memory, disk and network charts per node exist
  today, with history kept in memory only (see
  [Node overview](/docs/guides/node-monitoring/)).
- **Automation:** versioned scripts (shell, PowerShell, Python, .NET, PHP)
  run on a schedule or by hand on authorized nodes, and rules that respond to
  service and container failures, with one execution history.
- **Database management:** an instance-first view of SQL Server, PostgreSQL,
  MySQL, Redis, MongoDB, Cassandra and ClickHouse: inventory, configuration,
  query console, logs, backups and restore first, engine-specific replication
  and HA later.
- **Alerts and notifications:** alert rules on containers and jobs,
  escalation steps, grouping, recurring quiet hours, HTML email,
  provider-specific channels (native Discord, SMS), and Handlebars
  message templates. Alert rules on nodes and health checks, alarms with
  acknowledgement, delivery to webhooks, Slack-compatible chats and email, and
  silences and maintenance windows exist today (see
  [Alerts](/docs/concepts/alerts/) and
  [Silences and maintenance windows](/docs/concepts/silences/)).

- **Git-backed stacks:** a stack that follows a Git branch with a deploy key and redeploys on push. Until then, use a [deploy webhook](/docs/guides/deploy-webhooks/) from CI.
