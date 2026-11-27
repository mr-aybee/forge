//! The App Store catalog.
//!
//! Every installable package is declared here, together with the shell
//! recipe that installs it. The catalog is compiled into both the control
//! plane (to list packages and validate requests) and the node's root helper
//! (to run recipes). The control plane can therefore only ask for
//! "package X, version Y, these typed parameters": it can never send a
//! script, and the helper re-validates everything before running anything.
//!
//! # Recipe contract
//! A recipe is a POSIX shell script appended to [`PRELUDE`]. It is run as
//! root with `sh -s -- <action>`, a clean environment and:
//! * `FORGE_PKG_ID`, `FORGE_PKG_VERSION` — the validated package and version;
//! * `FORGE_P_<KEY>` — each declared parameter (validated, upper-cased key);
//! * the helper's functions from the prelude (`apt_install`, `svc`, `pkg_env`…).
//!
//! Actions: `install` (idempotent), `uninstall`, `start`, `stop`, `restart`,
//! `status` (prints `running`, `stopped` or `not-installed` as its last line).
//!
//! # Paths (same on every node)
//! * `/opt/forge/pkgs/<id>/<version>/` — files a package brings itself;
//! * `/var/lib/forge/pkgs/<id>/` — its data;
//! * `/etc/forge/pkgs/<id>.env` — `FORGE_PKG_<ID>_*` variables apps can use
//!   (mode 0600; generated passwords never leave the node).

pub mod apps;
pub mod compose_native;
pub mod container_opts;
pub mod deploy;
pub mod docker_ops;
pub mod docker_tls;
pub mod fabric;
pub mod health;
pub mod ingress;
pub mod integration;
pub mod names;
pub mod placement;
pub mod sites;
pub mod stack_doc;
pub mod stack_templates;
pub mod stacks;
pub mod storage;
pub mod swarm;
pub mod swarm_rollout;
pub mod swarm_service;
pub mod swarm_stack;
pub mod versions;
#[cfg(feature = "yaml")]
pub mod yaml;

use std::collections::BTreeMap;

use serde::Serialize;

pub use apps::{AppSpec, Runtime};
pub use sites::SiteSpec;

/// Shared shell functions available to every recipe.
pub const PRELUDE: &str = include_str!("../recipes/_prelude.sh");

/// The Windows recipe runner (PowerShell, winget), driven by the package's
/// `winget` ids and the validated variables.
pub const WINGET_RUNNER: &str = include_str!("../recipes/_winget.ps1");

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Category {
    Runtimes,
    Web,
    Databases,
    Storage,
    Containers,
    Dns,
    Tools,
    Messaging,
    Monitoring,
    DevOps,
}

/// How the package runs once installed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// OS packages / files managed by the node's service manager.
    Native,
    /// A container on the node's Docker engine (requires `docker`).
    Container,
    /// Changes the node's role, e.g. joins it to an orchestrator.
    Plugin,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ParamKind {
    /// TCP port, 1–65535.
    Port,
    /// One of a fixed list.
    Choice { options: &'static [&'static str] },
    /// `true` / `false`.
    Bool,
    /// Comma-separated list of lowercase identifiers (`[a-z0-9_-]`).
    List,
    /// A hostname or IP address, or empty.
    Address,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Param {
    pub key: &'static str,
    pub label: &'static str,
    pub kind: ParamKind,
    pub default: &'static str,
    pub help: &'static str,
    /// Must be `true` to install (licence acceptance).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub must_accept: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Package {
    pub id: &'static str,
    pub name: &'static str,
    pub category: Category,
    pub kind: Kind,
    pub summary: &'static str,
    pub homepage: &'static str,
    pub license: &'static str,
    /// Offered versions; the first one is the default.
    pub versions: &'static [&'static str],
    pub params: &'static [Param],
    /// Packages that must be installed on the same node first.
    pub requires: &'static [&'static str],
    /// Ports the package listens on by default (informational).
    pub ports: &'static [u16],
    /// Supported operating systems (`linux` today).
    pub os: &'static [&'static str],
    /// Free text shown on the card: what the install does and where things go.
    pub notes: &'static str,
    /// Product slug on endoflife.date (vendor release lines, LTS, EOL), or "".
    pub upstream: &'static str,
    /// Vendor release notes / changelog.
    pub release_notes: &'static str,
    /// Windows: winget package ids to try, `|`-separated; `{line}`, `{major}`
    /// and `{flavor}` are expanded. Empty: not available on Windows.
    pub winget: &'static str,
    /// Windows service the package runs (`{line}` expanded), or empty.
    pub winget_service: &'static str,
    #[serde(skip)]
    pub recipe: &'static str,
}

/// Lifecycle actions a recipe implements.
pub const ACTIONS: &[&str] = &["install", "uninstall", "start", "stop", "restart", "status", "versions", "upgrade"];

/// Actions that only read state on the node.
pub const READ_ONLY_ACTIONS: &[&str] = &["status", "versions"];

/// Reserved parameter: the exact OS package build to install or upgrade to.
pub const BUILD_PARAM: &str = "build";

/// Whether the recipe installs from a package repository, so Forge can
/// report and apply updates (`versions` / `upgrade`).
#[must_use]
pub fn updatable(pkg: &Package) -> bool {
    pkg.recipe.contains("OS_PKG=") || !pkg.winget.is_empty()
}

const fn p(
    key: &'static str,
    label: &'static str,
    kind: ParamKind,
    default: &'static str,
    help: &'static str,
) -> Param {
    Param { key, label, kind, default, help, must_accept: false }
}

const PORT: ParamKind = ParamKind::Port;
const BIND: Param = p(
    "bind",
    "Listen on",
    ParamKind::Choice { options: &["localhost", "all"] },
    "localhost",
    "`all` exposes the service on every interface; keep `localhost` unless other nodes connect to it.",
);

/// The full catalog, in display order.
pub static CATALOG: &[Package] = &[
    // ------------------------------------------------------------ runtimes
    Package {
        id: "php",
        name: "PHP-FPM",
        category: Category::Runtimes,
        kind: Kind::Native,
        summary: "PHP with FPM for Nginx; several versions can be installed side by side.",
        homepage: "https://www.php.net",
        license: "PHP-3.01",
        versions: &["8.3", "8.4", "8.2", "8.1", "7.4"],
        params: &[p(
            "extensions",
            "Extensions",
            ParamKind::List,
            "mbstring,xml,curl,zip,intl,gd,mysql,pgsql,redis,opcache",
            "Comma-separated PHP extension names.",
        )],
        requires: &[],
        ports: &[],
        os: &["linux"],
        notes: "Sury on Debian/Ubuntu, Remi on RHEL/Rocky/Alma; versions install side by side. FPM socket: /run/php/php<version>-fpm.sock (Debian) or /var/opt/remi/php<vv>/run/php-fpm/www.sock (RHEL).",
        upstream: "php",
        release_notes: "https://www.php.net/ChangeLog-8.php",
        winget: "",
        winget_service: "",
        recipe: include_str!("../recipes/php.sh"),
    },
    Package {
        id: "python",
        name: "Python",
        category: Category::Runtimes,
        kind: Kind::Native,
        summary: "Python 3 with venv and pip for WSGI/ASGI apps (gunicorn, uvicorn).",
        homepage: "https://www.python.org",
        license: "PSF-2.0",
        versions: &["system"],
        params: &[],
        requires: &[],
        ports: &[],
        os: &["linux"],
        notes: "The distribution's Python 3 plus python3-venv and pip; apps get their own virtualenv.",
        upstream: "python",
        release_notes: "https://docs.python.org/3/whatsnew/changelog.html",
        winget: "",
        winget_service: "",
        recipe: include_str!("../recipes/python.sh"),
    },
    Package {
        id: "nodejs",
        name: "Node.js",
        category: Category::Runtimes,
        kind: Kind::Native,
        summary: "Node.js LTS from NodeSource.",
        homepage: "https://nodejs.org",
        license: "MIT",
        versions: &["22", "20", "24"],
        params: &[],
        requires: &[],
        ports: &[],
        os: &["linux", "windows"],
        notes: "Installs one major version system-wide from NodeSource (deb and rpm repositories).",
        upstream: "nodejs",
        release_notes: "https://nodejs.org/en/blog/release",
        winget: "OpenJS.NodeJS.{line}|OpenJS.NodeJS.LTS|OpenJS.NodeJS",
        winget_service: "",
        recipe: include_str!("../recipes/nodejs.sh"),
    },
    Package {
        id: "java",
        name: "Java (Eclipse Temurin)",
        category: Category::Runtimes,
        kind: Kind::Native,
        summary: "OpenJDK builds from Adoptium for Spring Boot and other JVM apps.",
        homepage: "https://adoptium.net",
        license: "GPL-2.0-with-classpath-exception",
        versions: &["21", "17", "25"],
        params: &[],
        requires: &[],
        ports: &[],
        os: &["linux", "windows"],
        notes: "Installs temurin-<version>-jdk from Adoptium's deb or rpm repository.",
        upstream: "eclipse-temurin",
        release_notes: "https://adoptium.net/temurin/release-notes/",
        winget: "EclipseAdoptium.Temurin.{line}.JDK",
        winget_service: "",
        recipe: include_str!("../recipes/java.sh"),
    },
    Package {
        id: "dotnet",
        name: ".NET",
        category: Category::Runtimes,
        kind: Kind::Native,
        summary: "Cross-platform .NET runtime and ASP.NET Core (or the full SDK).",
        homepage: "https://dotnet.microsoft.com",
        license: "MIT",
        versions: &["8.0", "9.0", "10.0"],
        params: &[p(
            "flavor",
            "Install",
            ParamKind::Choice { options: &["aspnetcore", "runtime", "sdk"] },
            "aspnetcore",
            "`aspnetcore` runs web apps; `sdk` also builds them.",
        )],
        requires: &[],
        ports: &[],
        os: &["linux", "windows"],
        notes: "From the OS repositories where they carry the version (Microsoft's repository on Debian, the Ubuntu archive, RHEL AppStream), so apt/dnf update it. Otherwise Microsoft's dotnet-install.sh into /opt/forge/pkgs/dotnet/<version> (update by reinstalling).",
        upstream: "dotnet",
        release_notes: "https://github.com/dotnet/core/tree/main/release-notes",
        winget: "Microsoft.DotNet.{flavor}.{major}",
        winget_service: "",
        recipe: include_str!("../recipes/dotnet.sh"),
    },
    Package {
        id: "go",
        name: "Go",
        category: Category::Runtimes,
        kind: Kind::Native,
        summary: "The Go toolchain for building services on the node.",
        homepage: "https://go.dev",
        license: "BSD-3-Clause",
        versions: &["1.25.4", "1.24.10"],
        params: &[],
        requires: &[],
        ports: &[],
        os: &["linux"],
        notes: "Official tarball, SHA-256 checked against go.dev, unpacked to /opt/forge/pkgs/go/<version>. Not from a package repository: install a newer version to update.",
        upstream: "go",
        release_notes: "https://go.dev/doc/devel/release",
        winget: "",
        winget_service: "",
        recipe: include_str!("../recipes/go.sh"),
    },
    // ------------------------------------------------------------ web
    Package {
        id: "nginx",
        name: "Nginx",
        category: Category::Web,
        kind: Kind::Native,
        summary: "Web server and reverse proxy; turns a node into an edge node for Sites.",
        homepage: "https://nginx.org",
        license: "BSD-2-Clause",
        versions: &["stable"],
        params: &[],
        requires: &[],
        ports: &[80, 443],
        os: &["linux"],
        notes: "Distribution package (apt or dnf). Forge-managed site configs go to /etc/nginx/forge/. On RHEL, SELinux is set to let Nginx proxy and ports 80/443 are opened in firewalld.",
        upstream: "nginx",
        release_notes: "https://nginx.org/en/CHANGES",
        winget: "",
        winget_service: "",
        recipe: include_str!("../recipes/nginx.sh"),
    },
    // ------------------------------------------------------------ databases
    Package {
        id: "postgresql",
        name: "PostgreSQL",
        category: Category::Databases,
        kind: Kind::Native,
        summary: "The PostgreSQL server from the official PGDG repository.",
        homepage: "https://www.postgresql.org",
        license: "PostgreSQL",
        versions: &["17", "16", "15", "18"],
        params: &[p("port", "Port", PORT, "5432", ""), BIND],
        requires: &[],
        ports: &[5432],
        os: &["linux", "windows"],
        notes: "A random password for the `postgres` role is generated on the node and kept in /etc/forge/pkgs/postgresql.env.",
        upstream: "postgresql",
        release_notes: "https://www.postgresql.org/docs/release/",
        winget: "PostgreSQL.PostgreSQL.{line}",
        winget_service: "postgresql-x64-{line}",
        recipe: include_str!("../recipes/postgresql.sh"),
    },
    Package {
        id: "mariadb",
        name: "MariaDB",
        category: Category::Databases,
        kind: Kind::Native,
        summary: "MySQL-compatible database server (distribution package, apt or dnf).",
        homepage: "https://mariadb.org",
        license: "GPL-2.0",
        versions: &["distro"],
        params: &[p("port", "Port", PORT, "3306", ""), BIND],
        requires: &[],
        ports: &[3306],
        os: &["linux"],
        notes: "Creates an admin user `forge_admin` with a random password kept in /etc/forge/pkgs/mariadb.env.",
        upstream: "mariadb",
        release_notes: "https://mariadb.com/kb/en/release-notes/",
        winget: "",
        winget_service: "",
        recipe: include_str!("../recipes/mariadb.sh"),
    },
    Package {
        id: "redis",
        name: "Redis",
        category: Category::Databases,
        kind: Kind::Native,
        summary: "In-memory cache and message broker, password protected.",
        homepage: "https://redis.io",
        license: "AGPL-3.0 / RSALv2 / SSPLv1",
        versions: &["distro"],
        params: &[p("port", "Port", PORT, "6379", ""), BIND],
        requires: &[],
        ports: &[6379],
        os: &["linux"],
        notes: "requirepass is set to a random password kept in /etc/forge/pkgs/redis.env.",
        upstream: "redis",
        release_notes: "https://github.com/redis/redis/releases",
        winget: "",
        winget_service: "",
        recipe: include_str!("../recipes/redis.sh"),
    },
    Package {
        id: "mongodb",
        name: "MongoDB",
        category: Category::Databases,
        kind: Kind::Native,
        summary: "MongoDB Community Server from the official repository.",
        homepage: "https://www.mongodb.com",
        license: "SSPL-1.0",
        versions: &["8.0", "7.0"],
        params: &[p("port", "Port", PORT, "27017", ""), BIND],
        requires: &[],
        ports: &[27017],
        os: &["linux"],
        notes: "Authorization is enabled; the admin password is kept in /etc/forge/pkgs/mongodb.env.",
        upstream: "mongodb",
        release_notes: "https://www.mongodb.com/docs/manual/release-notes/",
        winget: "",
        winget_service: "",
        recipe: include_str!("../recipes/mongodb.sh"),
    },
    Package {
        id: "sqlserver",
        name: "SQL Server",
        category: Category::Databases,
        kind: Kind::Container,
        summary: "Microsoft SQL Server for Linux, run as a container.",
        homepage: "https://www.microsoft.com/sql-server",
        license: "Proprietary (Microsoft EULA)",
        versions: &["2022", "2025", "2019"],
        params: &[
            p("port", "Port", PORT, "1433", ""),
            p(
                "edition",
                "Edition",
                ParamKind::Choice { options: &["Developer", "Express", "Standard", "Enterprise"] },
                "Developer",
                "Standard and Enterprise need a licence.",
            ),
            Param {
                key: "accept_eula",
                label: "I accept the Microsoft SQL Server EULA",
                kind: ParamKind::Bool,
                default: "false",
                help: "https://go.microsoft.com/fwlink/?LinkId=746388",
                must_accept: true,
            },
        ],
        requires: &["docker"],
        ports: &[1433],
        os: &["linux"],
        notes: "Needs 2 GB RAM. The `sa` password is generated on the node; data lives in /var/lib/forge/pkgs/sqlserver. Runs as a container, so it is not tracked on the Updates page: reinstall to pull the newest cumulative update.",
        upstream: "mssqlserver",
        release_notes: "https://learn.microsoft.com/troubleshoot/sql/releases/download-and-install-latest-updates",
        winget: "",
        winget_service: "",
        recipe: include_str!("../recipes/sqlserver.sh"),
    },
    // ------------------------------------------------------------ storage
    Package {
        id: "rustfs",
        name: "RustFS",
        category: Category::Storage,
        kind: Kind::Container,
        summary: "S3-compatible object storage written in Rust.",
        homepage: "https://rustfs.com",
        license: "Apache-2.0",
        versions: &["latest"],
        params: &[p("port", "S3 API port", PORT, "9000", ""), p("console_port", "Console port", PORT, "9001", "")],
        requires: &["docker"],
        ports: &[9000, 9001],
        os: &["linux"],
        notes: "Access and secret keys are generated on the node and kept in /etc/forge/pkgs/rustfs.env. Runs as a container: reinstall to update.",
        upstream: "",
        release_notes: "",
        winget: "",
        winget_service: "",
        recipe: include_str!("../recipes/rustfs.sh"),
    },
    // ------------------------------------------------------------ containers
    Package {
        id: "docker",
        name: "Docker Engine",
        category: Category::Containers,
        kind: Kind::Native,
        summary: "Container runtime. Forge manages containers on nodes that have it.",
        homepage: "https://www.docker.com",
        license: "Apache-2.0",
        versions: &["stable"],
        params: &[],
        requires: &[],
        ports: &[],
        os: &["linux"],
        notes: "Docker CE from download.docker.com (deb or rpm), with the compose plugin. Updating restarts the daemon.",
        upstream: "docker-engine",
        release_notes: "https://docs.docker.com/engine/release-notes/",
        winget: "",
        winget_service: "",
        recipe: include_str!("../recipes/docker.sh"),
    },
    // ------------------------------------------------------------ dns
    Package {
        id: "powerdns",
        name: "PowerDNS",
        category: Category::Dns,
        kind: Kind::Native,
        summary: "Authoritative DNS server with an HTTP API, used by Forge's DNS management.",
        homepage: "https://www.powerdns.com",
        license: "GPL-2.0",
        versions: &["distro"],
        params: &[p("api_port", "API port", PORT, "8081", "")],
        requires: &[],
        ports: &[53, 8081],
        os: &["linux"],
        notes: "SQLite backend in /var/lib/forge/pkgs/powerdns; the API key is kept in /etc/forge/pkgs/powerdns.env.",
        upstream: "",
        release_notes: "https://doc.powerdns.com/authoritative/changelog/",
        winget: "",
        winget_service: "",
        recipe: include_str!("../recipes/powerdns.sh"),
    },
    // ------------------------------------------------------------ tools
    Package {
        id: "certbot",
        name: "Certbot",
        category: Category::Tools,
        kind: Kind::Native,
        summary: "Let's Encrypt certificates for Nginx sites (ACME HTTP-01).",
        homepage: "https://certbot.eff.org",
        license: "Apache-2.0",
        versions: &["distro"],
        params: &[],
        requires: &["nginx"],
        ports: &[],
        os: &["linux"],
        notes: "Distribution certbot (EPEL on RHEL) with the Nginx plugin; renewals run from its timer.",
        upstream: "",
        release_notes: "https://github.com/certbot/certbot/blob/main/certbot/CHANGELOG.md",
        winget: "",
        winget_service: "",
        recipe: include_str!("../recipes/certbot.sh"),
    },
    Package {
        id: "keepalived",
        name: "Keepalived",
        category: Category::Tools,
        kind: Kind::Native,
        summary: "Floating virtual IP for active/standby edge nodes (VRRP).",
        homepage: "https://www.keepalived.org",
        license: "GPL-2.0",
        versions: &["distro"],
        params: &[
            p("vip", "Virtual IP", ParamKind::Address, "", "Shared address, e.g. 10.0.0.50."),
            p("state", "Role", ParamKind::Choice { options: &["MASTER", "BACKUP"] }, "MASTER", ""),
            p("interface", "Interface", ParamKind::List, "eth0", "Network interface carrying the VIP."),
        ],
        requires: &[],
        ports: &[],
        os: &["linux"],
        notes: "Install on two edge nodes with the same VIP: one MASTER, one BACKUP.",
        upstream: "",
        release_notes: "https://www.keepalived.org/release-notes.html",
        winget: "",
        winget_service: "",
        recipe: include_str!("../recipes/keepalived.sh"),
    },
    // ------------------------------------------------------------ more databases
    Package {
        id: "mysql",
        name: "MySQL",
        category: Category::Databases,
        kind: Kind::Native,
        summary: "MySQL Community Server from Oracle's repositories.",
        homepage: "https://www.mysql.com",
        license: "GPL-2.0",
        versions: &["8.4", "8.0"],
        params: &[p("port", "Port", PORT, "3306", ""), BIND],
        requires: &[],
        ports: &[3306],
        os: &["linux"],
        notes: "8.4 is the LTS track. Creates `forge_admin` with a random password kept in /etc/forge/pkgs/mysql.env. Cannot share a node with MariaDB.",
        upstream: "mysql",
        release_notes: "https://dev.mysql.com/doc/relnotes/mysql/8.4/en/",
        winget: "",
        winget_service: "",
        recipe: include_str!("../recipes/mysql.sh"),
    },
    Package {
        id: "valkey",
        name: "Valkey",
        category: Category::Databases,
        kind: Kind::Native,
        summary: "Redis-compatible in-memory store (BSD licence), password protected.",
        homepage: "https://valkey.io",
        license: "BSD-3-Clause",
        versions: &["distro"],
        params: &[p("port", "Port", PORT, "6380", ""), BIND],
        requires: &[],
        ports: &[6380],
        os: &["linux"],
        notes: "Distribution package: Debian 13+, Ubuntu 24.10+, RHEL/Rocky/Alma with EPEL. Default port 6380 so it can run next to Redis.",
        upstream: "valkey",
        release_notes: "https://github.com/valkey-io/valkey/releases",
        winget: "",
        winget_service: "",
        recipe: include_str!("../recipes/valkey.sh"),
    },
    Package {
        id: "memcached",
        name: "Memcached",
        category: Category::Databases,
        kind: Kind::Native,
        summary: "Distributed memory cache.",
        homepage: "https://memcached.org",
        license: "BSD-3-Clause",
        versions: &["distro"],
        params: &[
            p("port", "Port", PORT, "11211", ""),
            p("memory_mb", "Memory (MB)", ParamKind::Choice { options: &["64", "256", "1024", "4096"] }, "256", ""),
            BIND,
        ],
        requires: &[],
        ports: &[11211],
        os: &["linux"],
        notes: "Distribution package; UDP is disabled.",
        upstream: "",
        release_notes: "https://github.com/memcached/memcached/wiki/ReleaseNotes",
        winget: "",
        winget_service: "",
        recipe: include_str!("../recipes/memcached.sh"),
    },
    Package {
        id: "clickhouse",
        name: "ClickHouse",
        category: Category::Databases,
        kind: Kind::Native,
        summary: "Column-oriented analytics database.",
        homepage: "https://clickhouse.com",
        license: "Apache-2.0",
        versions: &["lts", "stable"],
        params: &[p("port", "Native port", PORT, "9000", ""), p("http_port", "HTTP port", PORT, "8123", ""), BIND],
        requires: &[],
        ports: &[9000, 8123],
        os: &["linux"],
        notes: "From packages.clickhouse.com. The `default` user gets a random password kept in /etc/forge/pkgs/clickhouse.env.",
        upstream: "clickhouse",
        release_notes: "https://clickhouse.com/docs/whats-new/changelog",
        winget: "",
        winget_service: "",
        recipe: include_str!("../recipes/clickhouse.sh"),
    },
    Package {
        id: "influxdb",
        name: "InfluxDB 2",
        category: Category::Databases,
        kind: Kind::Native,
        summary: "Time-series database (OSS v2) with an operator token for apps.",
        homepage: "https://www.influxdata.com",
        license: "MIT",
        versions: &["2"],
        params: &[p("port", "Port", PORT, "8086", ""), BIND],
        requires: &[],
        ports: &[8086],
        os: &["linux"],
        notes: "From repos.influxdata.com. Sets up user `forge_admin`, org `forge`, bucket `default`; password and token stay on the node.",
        upstream: "influxdb",
        release_notes: "https://docs.influxdata.com/influxdb/v2/reference/release-notes/influxdb/",
        winget: "",
        winget_service: "",
        recipe: include_str!("../recipes/influxdb.sh"),
    },
    Package {
        id: "elasticsearch",
        name: "Elasticsearch",
        category: Category::Databases,
        kind: Kind::Native,
        summary: "Search and analytics engine, single node with security on.",
        homepage: "https://www.elastic.co",
        license: "Elastic-2.0 / AGPL-3.0",
        versions: &["8", "9"],
        params: &[
            p("port", "HTTP port", PORT, "9200", ""),
            p(
                "heap",
                "Java heap",
                ParamKind::Choice { options: &["512m", "1g", "2g", "4g", "8g"] },
                "1g",
                "Half the node's RAM at most.",
            ),
            BIND,
        ],
        requires: &[],
        ports: &[9200],
        os: &["linux"],
        notes: "From artifacts.elastic.co. TLS and the `elastic` user are on; its password stays on the node. Needs 2 GB RAM or more.",
        upstream: "elasticsearch",
        release_notes: "https://www.elastic.co/guide/en/elasticsearch/reference/current/es-release-notes.html",
        winget: "",
        winget_service: "",
        recipe: include_str!("../recipes/elasticsearch.sh"),
    },
    Package {
        id: "opensearch",
        name: "OpenSearch",
        category: Category::Databases,
        kind: Kind::Native,
        summary: "Apache-licensed search and analytics engine, single node with security on.",
        homepage: "https://opensearch.org",
        license: "Apache-2.0",
        versions: &["2", "3"],
        params: &[
            p("port", "HTTP port", PORT, "9200", ""),
            p(
                "heap",
                "Java heap",
                ParamKind::Choice { options: &["512m", "1g", "2g", "4g", "8g"] },
                "1g",
                "Half the node's RAM at most.",
            ),
            BIND,
        ],
        requires: &[],
        ports: &[9200],
        os: &["linux"],
        notes: "From artifacts.opensearch.org. The `admin` password is generated and kept on the node. Needs 2 GB RAM or more.",
        upstream: "opensearch",
        release_notes: "https://github.com/opensearch-project/opensearch-build/tree/main/release-notes",
        winget: "",
        winget_service: "",
        recipe: include_str!("../recipes/opensearch.sh"),
    },
    // ------------------------------------------------------------ messaging
    Package {
        id: "rabbitmq",
        name: "RabbitMQ",
        category: Category::Messaging,
        kind: Kind::Native,
        summary: "Message broker with the management UI, from Team RabbitMQ's repositories.",
        homepage: "https://www.rabbitmq.com",
        license: "MPL-2.0",
        versions: &["stable"],
        params: &[
            p("port", "AMQP port", PORT, "5672", ""),
            p("ui_port", "Management UI port", PORT, "15672", ""),
            BIND,
        ],
        requires: &[],
        ports: &[5672, 15672],
        os: &["linux"],
        notes: "Erlang comes from Team RabbitMQ's repository too. Creates `forge_admin`; the guest user is removed.",
        upstream: "rabbitmq",
        release_notes: "https://www.rabbitmq.com/release-information",
        winget: "",
        winget_service: "",
        recipe: include_str!("../recipes/rabbitmq.sh"),
    },
    Package {
        id: "mosquitto",
        name: "Mosquitto",
        category: Category::Messaging,
        kind: Kind::Native,
        summary: "Eclipse Mosquitto MQTT broker, anonymous access off.",
        homepage: "https://mosquitto.org",
        license: "EPL-2.0",
        versions: &["distro"],
        params: &[p("port", "Port", PORT, "1883", ""), BIND],
        requires: &[],
        ports: &[1883],
        os: &["linux"],
        notes: "Distribution package (EPEL on RHEL). User `forge` with a password kept on the node.",
        upstream: "",
        release_notes: "https://mosquitto.org/ChangeLog.txt",
        winget: "",
        winget_service: "",
        recipe: include_str!("../recipes/mosquitto.sh"),
    },
    // ------------------------------------------------------------ more web
    Package {
        id: "apache",
        name: "Apache HTTP Server",
        category: Category::Web,
        kind: Kind::Native,
        summary: "The Apache web server (apache2 / httpd).",
        homepage: "https://httpd.apache.org",
        license: "Apache-2.0",
        versions: &["distro"],
        params: &[p("port", "Port", PORT, "8081", ""), BIND],
        requires: &[],
        ports: &[8081],
        os: &["linux"],
        notes: "Distribution package. Default port 8081 so it can run behind Forge's Nginx edge.",
        upstream: "apache-http-server",
        release_notes: "https://downloads.apache.org/httpd/CHANGES_2.4",
        winget: "",
        winget_service: "",
        recipe: include_str!("../recipes/apache.sh"),
    },
    Package {
        id: "haproxy",
        name: "HAProxy",
        category: Category::Web,
        kind: Kind::Native,
        summary: "TCP/HTTP load balancer. Forge installs and updates it; the configuration stays yours.",
        homepage: "https://www.haproxy.org",
        license: "GPL-2.0",
        versions: &["distro"],
        params: &[],
        requires: &[],
        ports: &[],
        os: &["linux"],
        notes: "Distribution package. Edit /etc/haproxy/haproxy.cfg on the node; updates reload it after a config check.",
        upstream: "haproxy",
        release_notes: "https://www.haproxy.org/download/",
        winget: "",
        winget_service: "",
        recipe: include_str!("../recipes/haproxy.sh"),
    },
    Package {
        id: "caddy",
        name: "Caddy",
        category: Category::Web,
        kind: Kind::Native,
        summary: "Web server with automatic HTTPS, from Caddy's official repositories.",
        homepage: "https://caddyserver.com",
        license: "Apache-2.0",
        versions: &["stable"],
        params: &[p("port", "Port", PORT, "8082", ""), BIND],
        requires: &[],
        ports: &[8082],
        os: &["linux"],
        notes: "Serves /etc/caddy/Caddyfile. Forge only moves the packaged welcome site to the chosen port.",
        upstream: "caddy",
        release_notes: "https://github.com/caddyserver/caddy/releases",
        winget: "",
        winget_service: "",
        recipe: include_str!("../recipes/caddy.sh"),
    },
    Package {
        id: "tomcat",
        name: "Apache Tomcat",
        category: Category::Web,
        kind: Kind::Native,
        summary: "Java servlet container (distribution package).",
        homepage: "https://tomcat.apache.org",
        license: "Apache-2.0",
        versions: &["distro"],
        params: &[p("port", "Port", PORT, "8080", ""), BIND],
        requires: &[],
        ports: &[8080],
        os: &["linux"],
        notes: "tomcat10 on Debian/Ubuntu, tomcat 9 on RHEL 9, with the distribution's Java. Deploy WARs to the webapps directory.",
        upstream: "tomcat",
        release_notes: "https://tomcat.apache.org/tomcat-10.1-doc/changelog.html",
        winget: "",
        winget_service: "",
        recipe: include_str!("../recipes/tomcat.sh"),
    },
    // ------------------------------------------------------------ monitoring
    Package {
        id: "grafana",
        name: "Grafana",
        category: Category::Monitoring,
        kind: Kind::Native,
        summary: "Dashboards for metrics and logs.",
        homepage: "https://grafana.com",
        license: "AGPL-3.0",
        versions: &["stable"],
        params: &[p("port", "Port", PORT, "3000", ""), BIND],
        requires: &[],
        ports: &[3000],
        os: &["linux"],
        notes: "From Grafana Labs' repositories. The `admin` password is generated and kept on the node.",
        upstream: "grafana",
        release_notes: "https://github.com/grafana/grafana/releases",
        winget: "",
        winget_service: "",
        recipe: include_str!("../recipes/grafana.sh"),
    },
    Package {
        id: "loki",
        name: "Grafana Loki",
        category: Category::Monitoring,
        kind: Kind::Native,
        summary: "Log store for Grafana.",
        homepage: "https://grafana.com/oss/loki/",
        license: "AGPL-3.0",
        versions: &["stable"],
        params: &[p("port", "Port", PORT, "3100", ""), BIND],
        requires: &[],
        ports: &[3100],
        os: &["linux"],
        notes: "From Grafana Labs' repositories, single binary with local storage.",
        upstream: "",
        release_notes: "https://github.com/grafana/loki/releases",
        winget: "",
        winget_service: "",
        recipe: include_str!("../recipes/loki.sh"),
    },
    Package {
        id: "prometheus",
        name: "Prometheus",
        category: Category::Monitoring,
        kind: Kind::Native,
        summary: "Metrics server and time-series database.",
        homepage: "https://prometheus.io",
        license: "Apache-2.0",
        versions: &["distro"],
        params: &[p("port", "Port", PORT, "9090", ""), BIND],
        requires: &[],
        ports: &[9090],
        os: &["linux"],
        notes: "Distribution package (Debian/Ubuntu; EPEL on RHEL where packaged). Add scrape targets in /etc/prometheus/prometheus.yml.",
        upstream: "prometheus",
        release_notes: "https://github.com/prometheus/prometheus/releases",
        winget: "",
        winget_service: "",
        recipe: include_str!("../recipes/prometheus.sh"),
    },
    Package {
        id: "node-exporter",
        name: "Node exporter",
        category: Category::Monitoring,
        kind: Kind::Native,
        summary: "Host metrics for Prometheus.",
        homepage: "https://prometheus.io",
        license: "Apache-2.0",
        versions: &["distro"],
        params: &[p("port", "Port", PORT, "9100", ""), BIND],
        requires: &[],
        ports: &[9100],
        os: &["linux"],
        notes: "Distribution package (EPEL on RHEL). Bind to all interfaces so Prometheus on another node can scrape it.",
        upstream: "",
        release_notes: "https://github.com/prometheus/node_exporter/releases",
        winget: "",
        winget_service: "",
        recipe: include_str!("../recipes/node-exporter.sh"),
    },
    Package {
        id: "zabbix-agent",
        name: "Zabbix agent 2",
        category: Category::Monitoring,
        kind: Kind::Native,
        summary: "Reports this node to your Zabbix server.",
        homepage: "https://www.zabbix.com",
        license: "AGPL-3.0",
        versions: &["7.0"],
        params: &[p("server", "Zabbix server", ParamKind::Address, "", "Address of your Zabbix server or proxy.")],
        requires: &[],
        ports: &[10050],
        os: &["linux"],
        notes: "From repo.zabbix.com (7.0 LTS). Opens port 10050 for the server's checks.",
        upstream: "zabbix",
        release_notes: "https://www.zabbix.com/release_notes",
        winget: "",
        winget_service: "",
        recipe: include_str!("../recipes/zabbix-agent.sh"),
    },
    // ------------------------------------------------------------ devops
    Package {
        id: "jenkins",
        name: "Jenkins",
        category: Category::DevOps,
        kind: Kind::Native,
        summary: "Jenkins LTS automation server.",
        homepage: "https://www.jenkins.io",
        license: "MIT",
        versions: &["lts"],
        params: &[p("port", "Port", PORT, "8085", ""), BIND],
        requires: &[],
        ports: &[8085],
        os: &["linux"],
        notes: "From pkg.jenkins.io with the distribution's Java 17/21. The setup wizard's password is in /var/lib/jenkins/secrets/initialAdminPassword on the node.",
        upstream: "jenkins",
        release_notes: "https://www.jenkins.io/changelog-stable/",
        winget: "",
        winget_service: "",
        recipe: include_str!("../recipes/jenkins.sh"),
    },
    Package {
        id: "gitlab-ce",
        name: "GitLab CE",
        category: Category::DevOps,
        kind: Kind::Native,
        summary: "Self-hosted Git, CI/CD and registry (Omnibus package).",
        homepage: "https://about.gitlab.com",
        license: "MIT",
        versions: &["stable"],
        params: &[
            p("port", "Port", PORT, "8929", ""),
            p("host", "Hostname", ParamKind::Address, "", "Name users reach it by; the node's hostname if empty."),
        ],
        requires: &[],
        ports: &[8929],
        os: &["linux"],
        notes: "From packages.gitlab.com. Needs 4 GB RAM. The root password is generated and kept on the node. Updates run its database migrations.",
        upstream: "gitlab",
        release_notes: "https://about.gitlab.com/releases/categories/releases/",
        winget: "",
        winget_service: "",
        recipe: include_str!("../recipes/gitlab-ce.sh"),
    },
];

/// Integrations planned for the store, shown as "coming soon".
pub static ROADMAP: &[(&str, &str)] = &[
    ("GitHub", "Deploy from a repository on push; webhooks and deploy keys."),
    ("Google Cloud Storage", "Upload and sync backups and build artefacts."),
    ("S3 backup", "Scheduled database and volume backups to any S3 endpoint (incl. RustFS)."),
    ("Mail (SMTP relay)", "Outbound mail for services."),
    ("Traefik / YARP", "Alternative reverse proxies to Nginx."),
    ("Web service firewall", "ModSecurity / Coraza rules for Sites."),
    (".NET Framework", "Windows Server hosting via IIS."),
];

#[must_use]
pub fn find(id: &str) -> Option<&'static Package> {
    CATALOG.iter().find(|p| p.id == id)
}

/// A request that passed validation: what the helper will run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Validated {
    pub package: &'static Package,
    pub version: &'static str,
    pub action: &'static str,
    /// Environment for the recipe (`FORGE_PKG_*`, `FORGE_P_*`).
    pub env: BTreeMap<String, String>,
}

fn valid_ident_list(v: &str) -> bool {
    v.len() <= 512
        && v.split(',').all(|s| {
            !s.is_empty()
                && s.len() <= 32
                && s.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
        })
}

fn valid_address(v: &str) -> bool {
    v.is_empty()
        || (v.len() <= 253
            && !v.starts_with(['-', '.'])
            && v.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b':')))
}

impl Param {
    fn check(&self, v: &str) -> Result<(), String> {
        let ok = match self.kind {
            ParamKind::Port => v.parse::<u16>().is_ok_and(|n| n > 0),
            ParamKind::Choice { options } => options.contains(&v),
            ParamKind::Bool => matches!(v, "true" | "false"),
            ParamKind::List => valid_ident_list(v),
            ParamKind::Address => valid_address(v),
        };

        if !ok {
            return Err(format!("invalid value for `{}`", self.key));
        }

        if self.must_accept && v != "true" {
            return Err(format!("`{}` must be accepted", self.label));
        }

        Ok(())
    }
}

/// Validates a request against the catalog. Unknown packages, versions,
/// actions and parameters are rejected; missing parameters take defaults.
pub fn validate(
    package: &str,
    version: &str,
    action: &str,
    params: &BTreeMap<String, String>,
) -> Result<Validated, String> {
    let pkg = find(package).ok_or_else(|| format!("unknown package `{package}`"))?;
    let version = *pkg
        .versions
        .iter()
        .find(|v| **v == version)
        .ok_or_else(|| format!("{} has no version `{version}`", pkg.name))?;
    let action = *ACTIONS.iter().find(|a| **a == action).ok_or_else(|| format!("unknown action `{action}`"))?;
    let build = params.get(BUILD_PARAM);
    if let Some(b) = build {
        if !matches!(action, "install" | "upgrade") {
            return Err(format!("`{BUILD_PARAM}` only applies to install and upgrade"));
        }

        if !versions::valid_build(b) {
            return Err(format!("invalid build `{b}`"));
        }
    }

    if let Some(k) =
        params.keys().find(|k| k.as_str() != BUILD_PARAM && !pkg.params.iter().any(|p| p.key == k.as_str()))
    {
        return Err(format!("{} has no parameter `{k}`", pkg.name));
    }

    let mut env = BTreeMap::new();
    env.insert("FORGE_PKG_ID".to_owned(), pkg.id.to_owned());
    env.insert("FORGE_PKG_VERSION".to_owned(), version.to_owned());

    if let Some(b) = build {
        env.insert("FORGE_PKG_BUILD".to_owned(), b.clone());
    }

    for p in pkg.params {
        let v = params.get(p.key).map_or(p.default, String::as_str);

        // Lifecycle actions other than install reuse what was installed;
        // licence acceptance only gates installation.
        if action == "install" || !p.must_accept {
            p.check(v)?;
        }

        env.insert(format!("FORGE_P_{}", p.key.to_ascii_uppercase()), v.to_owned());
    }

    Ok(Validated { package: pkg, version, action, env })
}

/// Environment for the Windows runner: the validated variables plus the
/// package's winget ids and the action. `None` if the package has no
/// Windows install.
#[must_use]
pub fn winget_env(v: &Validated) -> Option<BTreeMap<String, String>> {
    if v.package.winget.is_empty() {
        return None;
    }

    let mut env = v.env.clone();
    env.insert("FORGE_WINGET".to_owned(), v.package.winget.to_owned());
    env.insert("FORGE_WINGET_SERVICE".to_owned(), v.package.winget_service.to_owned());
    env.insert("FORGE_ACTION".to_owned(), v.action.to_owned());

    Some(env)
}

/// The full script the helper runs: prelude + recipe.
#[must_use]
pub fn script(pkg: &Package) -> String {
    format!("{PRELUDE}\n# ---- recipe: {} ----\n{}\nforge_dispatch \"$@\"\n", pkg.id, pkg.recipe)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    /// Installer secrets go through a locked-down option file, never the
    /// command line (every process list shows command lines).
    #[test]
    fn the_winget_runner_keeps_secrets_off_the_command_line() {
        assert!(!WINGET_RUNNER.contains("--superpassword"), "PostgreSQL password on the command line");
        assert!(WINGET_RUNNER.contains("--optionfile"));
        let lock = WINGET_RUNNER.find("icacls $script:OptFile").unwrap();
        let write = WINGET_RUNNER.find("Set-Content -Path $script:OptFile").unwrap();
        assert!(lock < write, "the option file is locked down before the secret is written");
        assert!(WINGET_RUNNER.contains("finally {\n    if ($script:OptFile) { Remove-Item"), "removed after winget");
    }

    fn m(kv: &[(&str, &str)]) -> BTreeMap<String, String> {
        kv.iter().map(|(k, v)| ((*k).to_owned(), (*v).to_owned())).collect()
    }

    /// Two installs on one node (or unattended-upgrades) must not fail on the
    /// dpkg lock: every apt run waits for it (issue #17).
    #[test]
    fn every_apt_run_waits_for_the_dpkg_lock() {
        assert!(PRELUDE.contains(r#"APT_LOCK="-o DPkg::Lock::Timeout="#));
        let sources = std::iter::once(PRELUDE).chain(CATALOG.iter().map(|p| p.recipe));

        for src in sources {
            for line in src.lines().filter(|l| !l.trim_start().starts_with('#')) {
                for (i, _) in line.match_indices("apt-get ") {
                    let before = &line[..i];
                    let after = &line[i + "apt-get ".len()..];

                    if before.ends_with("step ") || before.ends_with("command -v ") {
                        continue;
                    }

                    assert!(after.starts_with("$APT_LOCK"), "apt-get without $APT_LOCK: {line}");
                }
            }
        }
    }

    #[test]
    fn catalog_is_consistent() {
        // sh rejects CRLF scripts (`set: Illegal option -`).
        assert!(!PRELUDE.contains('\r'), "the prelude has CRLF line endings");
        let mut ids = std::collections::HashSet::new();

        for p in CATALOG {
            assert!(ids.insert(p.id), "duplicate id {}", p.id);
            assert!(!p.versions.is_empty(), "{} has no versions", p.id);
            assert!(p.recipe.contains("pkg_install()"), "{} recipe lacks pkg_install", p.id);
            assert!(!p.recipe.contains('\r'), "{} recipe has CRLF line endings", p.id);

            for r in p.requires {
                assert!(find(r).is_some(), "{} requires unknown {r}", p.id);
            }

            for prm in p.params {
                prm.check(prm.default).or_else(|e| if prm.must_accept { Ok(()) } else { Err(e) }).unwrap();
            }
        }
    }

    #[test]
    fn validation_rejects_anything_unexpected() {
        assert!(validate("postgresql", "17", "install", &m(&[("port", "5433")])).is_ok());
        assert!(validate("nope", "1", "install", &m(&[])).is_err());
        assert!(validate("postgresql", "9.6", "install", &m(&[])).is_err());
        assert!(validate("postgresql", "17", "exec", &m(&[])).is_err());
        assert!(validate("postgresql", "17", "install", &m(&[("port", "5432; rm -rf /")])).is_err());
        assert!(validate("postgresql", "17", "install", &m(&[("evil", "1")])).is_err());
        assert!(validate("php", "8.3", "install", &m(&[("extensions", "gd,$(id)")])).is_err());
        let pinned = validate("postgresql", "17", "upgrade", &m(&[("build", "17.6-1.pgdg24.04+1")])).unwrap();
        assert_eq!(pinned.env["FORGE_PKG_BUILD"], "17.6-1.pgdg24.04+1");
        assert!(validate("postgresql", "17", "upgrade", &m(&[("build", "17; id")])).is_err());
        assert!(validate("postgresql", "17", "start", &m(&[("build", "17.6")])).is_err());
        assert!(updatable(find("postgresql").unwrap()));
        assert!(!updatable(find("go").unwrap()));
    }

    #[test]
    fn licences_must_be_accepted_to_install() {
        assert!(validate("sqlserver", "2022", "install", &m(&[])).is_err());
        let ok = validate("sqlserver", "2022", "install", &m(&[("accept_eula", "true")])).unwrap();
        assert_eq!(ok.env["FORGE_P_ACCEPT_EULA"], "true");
        assert_eq!(ok.env["FORGE_P_PORT"], "1433");
        assert!(validate("sqlserver", "2022", "status", &m(&[])).is_ok());
    }

    #[test]
    fn windows_runner_gets_winget_ids() {
        let v = validate("dotnet", "8.0", "versions", &m(&[("flavor", "sdk")])).unwrap();
        let env = winget_env(&v).unwrap();
        assert_eq!(env["FORGE_WINGET"], "Microsoft.DotNet.{flavor}.{major}");
        assert_eq!(env["FORGE_ACTION"], "versions");
        assert_eq!(env["FORGE_P_FLAVOR"], "sdk");
        assert!(winget_env(&validate("redis", "distro", "status", &m(&[])).unwrap()).is_none());
        assert!(updatable(find("postgresql").unwrap()));
    }

    #[test]
    fn manifests_serialize_without_recipes() {
        let v = serde_json::to_value(CATALOG).unwrap();
        assert!(v[0].get("recipe").is_none());
        assert_eq!(v[0]["category"], "runtimes");
    }
}
