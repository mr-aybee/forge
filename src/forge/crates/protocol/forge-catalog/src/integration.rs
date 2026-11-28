//! How each App Store package fits into Forge: what it is used for, what it
//! exports to services, the services and paths it owns on the node, and
//! the Forge features that build on it. Shown on the package's page.

use serde::Serialize;

/// A variable written to `/etc/forge/pkgs/<id>.env`.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct Export {
    pub name: &'static str,
    pub meaning: &'static str,
    /// Generated on the node and kept there (never sent to Forge).
    pub secret: bool,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct Integration {
    /// One paragraph: why you install it and what Forge does with it.
    pub role: &'static str,
    pub exports: &'static [Export],
    /// systemd units or containers it runs.
    pub services: &'static [&'static str],
    /// `(path, what lives there)`.
    pub paths: &'static [(&'static str, &'static str)],
    /// Forge features that use it.
    pub used_by: &'static [&'static str],
    /// How a service uses it, step by step.
    pub usage: &'static [&'static str],
}

const RUNTIME_USAGE: &[&str] = &[
    "Add it under Runtime dependencies in a service's Configuration tab, with the version you need.",
    "On Deploy, Forge installs it on every node of the app that lacks it and waits for the install.",
    "The app's launcher puts it on PATH, so the start command can call it directly.",
];
const DB_USAGE: &[&str] = &[
    "Install it on the node that should hold the data (bind to localhost unless other nodes connect).",
    "In the service's Configuration → Package variables, select this package.",
    "Read the FORGE_PKG_* variables in the app (or map them: DATABASE_URL=${FORGE_PKG_…_URL} in your code).",
];

const SERVICE_USAGE: &[&str] = &[
    "Install it on the node that should run it; bind to all interfaces only if other nodes or users connect.",
    "Generated credentials stay in /etc/forge/pkgs/<package>.env on the node.",
    "To publish it under a domain, add an Ingress endpoint with a node target on its port.",
];

/// Integration notes for a package id.
#[must_use]
pub fn integration(id: &str) -> Option<Integration> {
    Some(match id {
        "python" => Integration {
            role: "The system Python 3 with venv and pip, for services and jobs written in Python (Django, FastAPI, Flask, workers, scripts).",
            exports: &[Export { name: "FORGE_PKG_PYTHON_BIN", meaning: "Path of python3", secret: false }],
            services: &[],
            paths: &[
                ("/usr/bin/python3", "interpreter"),
                ("/srv/forge/apps/<ns>/<app>/venv", "create a venv per app here"),
            ],
            used_by: &["Services → runtime dependencies"],
            usage: RUNTIME_USAGE,
        },
        "nodejs" => Integration {
            role: "Node.js from NodeSource for JavaScript and TypeScript services (Express, Next.js server, workers).",
            exports: &[Export { name: "FORGE_PKG_NODEJS_BIN", meaning: "Path of node", secret: false }],
            services: &[],
            paths: &[("/usr/bin/node", "runtime"), ("/usr/bin/npm", "package manager")],
            used_by: &["Services → runtime dependencies"],
            usage: RUNTIME_USAGE,
        },
        "java" => Integration {
            role: "Eclipse Temurin JDK for Spring Boot and other JVM services.",
            exports: &[
                Export {
                    name: "FORGE_PKG_JAVA_HOME",
                    meaning: "JDK home (the launcher sets JAVA_HOME from it)",
                    secret: false,
                },
                Export { name: "FORGE_PKG_JAVA_BIN", meaning: "Path of java", secret: false },
            ],
            services: &[],
            paths: &[("/usr/lib/jvm/temurin-<version>-jdk-<arch>", "JDK")],
            used_by: &["Services → runtime dependencies"],
            usage: RUNTIME_USAGE,
        },
        "dotnet" => Integration {
            role: ".NET from Microsoft: the ASP.NET Core runtime for web apps, the base runtime for workers, or the SDK to build on the node. Several versions install side by side.",
            exports: &[
                Export {
                    name: "FORGE_PKG_DOTNET_HOME",
                    meaning: "Install directory (the launcher sets DOTNET_ROOT)",
                    secret: false,
                },
                Export { name: "FORGE_PKG_DOTNET_BIN", meaning: "Path of dotnet", secret: false },
            ],
            services: &[],
            paths: &[
                ("/opt/forge/pkgs/dotnet/<version>", "runtime or SDK"),
                ("/usr/local/bin/dotnet", "link to the last installed version"),
            ],
            used_by: &["Services → runtime dependencies (choose runtime, ASP.NET Core or SDK)"],
            usage: RUNTIME_USAGE,
        },
        "go" => Integration {
            role: "The Go toolchain, to build Go services on the node. A compiled Go binary needs no runtime: choose None for it.",
            exports: &[
                Export { name: "FORGE_PKG_GO_HOME", meaning: "GOROOT (the launcher sets it)", secret: false },
                Export { name: "FORGE_PKG_GO_BIN", meaning: "Path of go", secret: false },
            ],
            services: &[],
            paths: &[("/opt/forge/pkgs/go/<version>", "toolchain")],
            used_by: &["Services → runtime dependencies"],
            usage: RUNTIME_USAGE,
        },
        "php" => Integration {
            role: "PHP-FPM for PHP services behind Nginx. Versions install side by side; each has its own FPM pool and socket.",
            exports: &[
                Export {
                    name: "FORGE_PKG_PHP_BIN_<v>",
                    meaning: "Path of php<version>, e.g. FORGE_PKG_PHP_BIN_8_3",
                    secret: false,
                },
                Export {
                    name: "FORGE_PKG_PHP_FPM_SOCKET_<v>",
                    meaning: "FPM socket for Nginx (fastcgi_pass unix:…)",
                    secret: false,
                },
            ],
            services: &["php<version>-fpm"],
            paths: &[("/etc/php/<version>/fpm", "FPM configuration"), ("/run/php/php<version>-fpm.sock", "FPM socket")],
            used_by: &[
                "Services → runtime dependencies (the chosen version answers to `php`)",
                "Ingress endpoints (custom block: fastcgi_pass to the socket)",
            ],
            usage: &[
                "Add PHP and its version under Runtime dependencies, or install it here for an Nginx site.",
                "For a site, point the endpoint's custom block at the FPM socket of that version.",
                "Extensions are a parameter: reinstall with a different list to change them.",
            ],
        },
        "nginx" => Integration {
            role: "The web server and reverse proxy behind Ingress. Forge writes its sites and global settings; you rarely edit Nginx by hand.",
            exports: &[
                Export { name: "FORGE_PKG_NGINX_BIN", meaning: "Path of nginx", secret: false },
                Export {
                    name: "FORGE_PKG_NGINX_SITES_DIR",
                    meaning: "Directory of Forge-managed sites",
                    secret: false,
                },
            ],
            services: &["nginx"],
            paths: &[
                ("/etc/nginx/forge/sites", "one file per Ingress endpoint (managed)"),
                ("/etc/nginx/forge/custom", "your own snippets"),
                ("/etc/nginx/conf.d/forge-global.conf", "global settings from the cluster's ingress profile"),
            ],
            used_by: &[
                "Ingress: edge nodes, profiles (standalone, active-standby, active-active) and endpoints",
                "Cluster setup wizard",
            ],
            usage: &[
                "Install it on the nodes that receive traffic, or let the cluster wizard do it.",
                "Configure layouts in Ingress → Profile and domains in Ingress → Endpoints.",
                "Every change is tested with nginx -t and rolled back if it fails.",
            ],
        },
        "keepalived" => Integration {
            role: "VRRP failover: moves a virtual IP between Nginx edges so clients keep one address when an edge fails.",
            exports: &[Export { name: "FORGE_PKG_KEEPALIVED_VIP", meaning: "The virtual IP", secret: false }],
            services: &["keepalived"],
            paths: &[("/etc/keepalived/keepalived.conf", "rendered from the ingress profile")],
            used_by: &["Ingress profiles: active-standby and active-active with VRRP"],
            usage: &["Choose VRRP failover in Ingress → Profile; Forge installs and configures it on the edges."],
        },
        "certbot" => Integration {
            role: "Let's Encrypt certificates for Ingress endpoints that request automatic TLS.",
            exports: &[Export { name: "FORGE_PKG_CERTBOT_BIN", meaning: "Path of certbot", secret: false }],
            services: &["certbot.timer (renewal)"],
            paths: &[
                ("/etc/letsencrypt", "certificates and renewal configuration"),
                ("/etc/letsencrypt/renewal-hooks/deploy/forge-nginx-reload.sh", "reloads Nginx after a renewal"),
            ],
            used_by: &["Ingress endpoints with Let's Encrypt TLS"],
            usage: &[
                "Install it on every edge, then choose Let's Encrypt on an endpoint; renewals run on the node and reload Nginx.",
                "With several edges, each issues its own certificate; challenges are shared between the edges.",
            ],
        },
        "postgresql" => Integration {
            role: "PostgreSQL server for service data. Forge generates the superuser password on the node.",
            exports: &[
                Export { name: "FORGE_PKG_POSTGRESQL_HOST", meaning: "Host to connect to", secret: false },
                Export { name: "FORGE_PKG_POSTGRESQL_PORT", meaning: "Port", secret: false },
                Export { name: "FORGE_PKG_POSTGRESQL_USER", meaning: "Superuser (postgres)", secret: false },
                Export { name: "FORGE_PKG_POSTGRESQL_PASSWORD", meaning: "Generated superuser password", secret: true },
                Export { name: "FORGE_PKG_POSTGRESQL_URL", meaning: "Full connection URL", secret: true },
            ],
            services: &["postgresql"],
            paths: &[
                ("/etc/postgresql/<version>/main", "configuration"),
                ("/var/lib/postgresql/<version>/main", "data"),
            ],
            used_by: &["Services → package variables"],
            usage: DB_USAGE,
        },
        "mariadb" => Integration {
            role: "MariaDB (MySQL compatible) for service data, with a generated admin user.",
            exports: &[
                Export { name: "FORGE_PKG_MARIADB_HOST", meaning: "Host", secret: false },
                Export { name: "FORGE_PKG_MARIADB_PORT", meaning: "Port", secret: false },
                Export { name: "FORGE_PKG_MARIADB_USER", meaning: "Admin user (forge_admin)", secret: false },
                Export { name: "FORGE_PKG_MARIADB_PASSWORD", meaning: "Generated admin password", secret: true },
            ],
            services: &["mariadb"],
            paths: &[
                ("/etc/mysql/mariadb.conf.d/90-forge.cnf", "Forge settings (bind, port)"),
                ("/var/lib/mysql", "data"),
            ],
            used_by: &["Services → package variables"],
            usage: DB_USAGE,
        },
        "mongodb" => Integration {
            role: "MongoDB document database with authentication enabled and a generated admin user.",
            exports: &[
                Export { name: "FORGE_PKG_MONGODB_PORT", meaning: "Port", secret: false },
                Export { name: "FORGE_PKG_MONGODB_USER", meaning: "Admin user (forge_admin)", secret: false },
                Export { name: "FORGE_PKG_MONGODB_PASSWORD", meaning: "Generated admin password", secret: true },
            ],
            services: &["mongod"],
            paths: &[("/etc/mongod.conf", "configuration"), ("/var/lib/mongodb", "data")],
            used_by: &["Services → package variables"],
            usage: DB_USAGE,
        },
        "redis" => Integration {
            role: "Redis for caching, queues and sessions, password protected.",
            exports: &[
                Export { name: "FORGE_PKG_REDIS_HOST", meaning: "Host", secret: false },
                Export { name: "FORGE_PKG_REDIS_PORT", meaning: "Port", secret: false },
                Export { name: "FORGE_PKG_REDIS_PASSWORD", meaning: "Generated password", secret: true },
                Export { name: "FORGE_PKG_REDIS_URL", meaning: "redis:// URL with the password", secret: true },
            ],
            services: &["redis-server"],
            paths: &[("/etc/redis/redis.conf", "configuration")],
            used_by: &["Services → package variables"],
            usage: DB_USAGE,
        },
        "sqlserver" => Integration {
            role: "Microsoft SQL Server in a container (needs Docker Engine and accepting the licence).",
            exports: &[
                Export { name: "FORGE_PKG_SQLSERVER_HOST", meaning: "Host", secret: false },
                Export { name: "FORGE_PKG_SQLSERVER_PORT", meaning: "Port", secret: false },
                Export { name: "FORGE_PKG_SQLSERVER_USER", meaning: "sa", secret: false },
                Export { name: "FORGE_PKG_SQLSERVER_SA_PASSWORD", meaning: "Generated sa password", secret: true },
            ],
            services: &["container forge-pkg-sqlserver"],
            paths: &[("/var/lib/forge/pkgs/sqlserver", "data (mounted at /var/opt/mssql)")],
            used_by: &["Services → package variables"],
            usage: DB_USAGE,
        },
        "rustfs" => Integration {
            role: "S3-compatible object storage in a container, for uploads, backups and artefacts.",
            exports: &[
                Export { name: "FORGE_PKG_RUSTFS_ENDPOINT", meaning: "S3 endpoint URL", secret: false },
                Export { name: "FORGE_PKG_RUSTFS_ACCESS_KEY", meaning: "Generated access key", secret: true },
                Export { name: "FORGE_PKG_RUSTFS_SECRET_KEY", meaning: "Generated secret key", secret: true },
            ],
            services: &["container forge-pkg-rustfs"],
            paths: &[("/var/lib/forge/pkgs/rustfs", "objects (mounted at /data)")],
            used_by: &["Services → package variables"],
            usage: &[
                "Install it, select it under Package variables, and point your S3 client at the endpoint with the keys.",
            ],
        },
        "docker" => Integration {
            role: "Docker Engine: required for container services, containerised packages and the Containers tab of a node.",
            exports: &[
                Export { name: "FORGE_PKG_DOCKER_BIN", meaning: "Path of docker", secret: false },
                Export { name: "FORGE_PKG_DOCKER_SOCKET", meaning: "Engine socket", secret: false },
            ],
            services: &["docker"],
            paths: &[
                ("/var/lib/docker", "images, containers and volumes"),
                ("/var/run/docker.sock", "API socket (root only)"),
            ],
            used_by: &["Services (container type)", "Node → Containers", "SQL Server, RustFS"],
            usage: &[
                "Install it before deploying container services to a node; Forge talks to the engine through the root helper.",
            ],
        },
        "powerdns" => Integration {
            role: "Authoritative DNS server behind Forge's DNS page: zones and records for your domains, and records published by Ingress endpoints.",
            exports: &[
                Export { name: "FORGE_PKG_POWERDNS_API_URL", meaning: "HTTP API URL", secret: false },
                Export { name: "FORGE_PKG_POWERDNS_API_KEY", meaning: "Generated API key", secret: true },
            ],
            services: &["pdns"],
            paths: &[
                ("/etc/powerdns/pdns.d/90-forge.conf", "Forge settings"),
                ("/var/lib/forge/pkgs/powerdns/pdns.sqlite3", "zones and records"),
            ],
            used_by: &["DNS page (zones, records, name servers)", "Ingress endpoints → publish DNS"],
            usage: &["Install it on the node(s) that serve DNS, then manage zones on the DNS page."],
        },
        "mysql" => Integration {
            role: "MySQL Community Server for service data, with a generated admin user.",
            exports: &[
                Export { name: "FORGE_PKG_MYSQL_HOST", meaning: "Host", secret: false },
                Export { name: "FORGE_PKG_MYSQL_PORT", meaning: "Port", secret: false },
                Export { name: "FORGE_PKG_MYSQL_USER", meaning: "Admin user (forge_admin)", secret: false },
                Export { name: "FORGE_PKG_MYSQL_PASSWORD", meaning: "Generated password", secret: true },
            ],
            services: &["mysql / mysqld"],
            paths: &[
                ("/etc/mysql/mysql.conf.d/90-forge.cnf", "Forge settings (Debian)"),
                ("/etc/my.cnf.d/90-forge.cnf", "Forge settings (RHEL)"),
                ("/var/lib/mysql", "data"),
            ],
            used_by: &["Services → package variables"],
            usage: DB_USAGE,
        },
        "valkey" => Integration {
            role: "Valkey cache and broker, Redis-protocol compatible, password protected.",
            exports: &[
                Export { name: "FORGE_PKG_VALKEY_HOST", meaning: "Host", secret: false },
                Export { name: "FORGE_PKG_VALKEY_PORT", meaning: "Port", secret: false },
                Export { name: "FORGE_PKG_VALKEY_PASSWORD", meaning: "Generated password", secret: true },
                Export { name: "FORGE_PKG_VALKEY_URL", meaning: "redis:// connection URL", secret: true },
            ],
            services: &["valkey / valkey-server"],
            paths: &[("/etc/valkey/valkey.conf", "configuration")],
            used_by: &["Services → package variables"],
            usage: DB_USAGE,
        },
        "memcached" => Integration {
            role: "Memcached for caching sessions and query results.",
            exports: &[
                Export { name: "FORGE_PKG_MEMCACHED_HOST", meaning: "Host", secret: false },
                Export { name: "FORGE_PKG_MEMCACHED_PORT", meaning: "Port", secret: false },
            ],
            services: &["memcached"],
            paths: &[
                ("/etc/memcached.conf", "configuration (Debian)"),
                ("/etc/sysconfig/memcached", "configuration (RHEL)"),
            ],
            used_by: &["Services → package variables"],
            usage: DB_USAGE,
        },
        "clickhouse" => Integration {
            role: "ClickHouse for analytics; the default user gets a generated password.",
            exports: &[
                Export { name: "FORGE_PKG_CLICKHOUSE_HOST", meaning: "Host", secret: false },
                Export { name: "FORGE_PKG_CLICKHOUSE_PORT", meaning: "Native protocol port", secret: false },
                Export { name: "FORGE_PKG_CLICKHOUSE_HTTP_PORT", meaning: "HTTP port", secret: false },
                Export { name: "FORGE_PKG_CLICKHOUSE_USER", meaning: "User (default)", secret: false },
                Export { name: "FORGE_PKG_CLICKHOUSE_PASSWORD", meaning: "Generated password", secret: true },
            ],
            services: &["clickhouse-server"],
            paths: &[
                ("/etc/clickhouse-server/config.d/90-forge.xml", "listen address and ports"),
                ("/var/lib/clickhouse", "data"),
            ],
            used_by: &["Services → package variables"],
            usage: DB_USAGE,
        },
        "influxdb" => Integration {
            role: "InfluxDB 2 for metrics and events, set up with an org, a bucket and an operator token.",
            exports: &[
                Export { name: "FORGE_PKG_INFLUXDB_URL", meaning: "API URL", secret: true },
                Export { name: "FORGE_PKG_INFLUXDB_ORG", meaning: "Organisation (forge)", secret: false },
                Export { name: "FORGE_PKG_INFLUXDB_BUCKET", meaning: "Bucket (default)", secret: false },
                Export { name: "FORGE_PKG_INFLUXDB_USER", meaning: "Admin user", secret: false },
                Export { name: "FORGE_PKG_INFLUXDB_PASSWORD", meaning: "Generated password", secret: true },
                Export { name: "FORGE_PKG_INFLUXDB_TOKEN", meaning: "Operator API token", secret: true },
            ],
            services: &["influxdb"],
            paths: &[("/etc/influxdb/config.toml", "configuration"), ("/var/lib/influxdb", "data")],
            used_by: &["Services → package variables"],
            usage: DB_USAGE,
        },
        "elasticsearch" => Integration {
            role: "Elasticsearch search and analytics, single node with TLS and the elastic user.",
            exports: &[
                Export { name: "FORGE_PKG_ELASTICSEARCH_URL", meaning: "HTTPS URL", secret: true },
                Export { name: "FORGE_PKG_ELASTICSEARCH_USER", meaning: "User (elastic)", secret: false },
                Export { name: "FORGE_PKG_ELASTICSEARCH_PASSWORD", meaning: "Generated password", secret: true },
                Export { name: "FORGE_PKG_ELASTICSEARCH_CA_CERT", meaning: "CA certificate to trust", secret: false },
            ],
            services: &["elasticsearch"],
            paths: &[("/etc/elasticsearch", "configuration"), ("/var/lib/elasticsearch", "data")],
            used_by: &["Services → package variables"],
            usage: DB_USAGE,
        },
        "opensearch" => Integration {
            role: "OpenSearch search and analytics, single node with the security plugin.",
            exports: &[
                Export { name: "FORGE_PKG_OPENSEARCH_URL", meaning: "HTTPS URL", secret: true },
                Export { name: "FORGE_PKG_OPENSEARCH_USER", meaning: "User (admin)", secret: false },
                Export { name: "FORGE_PKG_OPENSEARCH_PASSWORD", meaning: "Generated password", secret: true },
            ],
            services: &["opensearch"],
            paths: &[("/etc/opensearch", "configuration"), ("/var/lib/opensearch", "data")],
            used_by: &["Services → package variables"],
            usage: DB_USAGE,
        },
        "rabbitmq" => Integration {
            role: "RabbitMQ message broker with the management UI and a generated admin user.",
            exports: &[
                Export { name: "FORGE_PKG_RABBITMQ_HOST", meaning: "Host", secret: false },
                Export { name: "FORGE_PKG_RABBITMQ_PORT", meaning: "AMQP port", secret: false },
                Export { name: "FORGE_PKG_RABBITMQ_USER", meaning: "User (forge_admin)", secret: false },
                Export { name: "FORGE_PKG_RABBITMQ_PASSWORD", meaning: "Generated password", secret: true },
                Export { name: "FORGE_PKG_RABBITMQ_URL", meaning: "amqp:// URL", secret: true },
                Export { name: "FORGE_PKG_RABBITMQ_UI", meaning: "Management UI", secret: false },
            ],
            services: &["rabbitmq-server"],
            paths: &[("/etc/rabbitmq/rabbitmq.conf", "configuration"), ("/var/lib/rabbitmq", "data")],
            used_by: &["Services → package variables"],
            usage: DB_USAGE,
        },
        "mosquitto" => Integration {
            role: "MQTT broker for devices and services; anonymous access is off.",
            exports: &[
                Export { name: "FORGE_PKG_MOSQUITTO_HOST", meaning: "Host", secret: false },
                Export { name: "FORGE_PKG_MOSQUITTO_PORT", meaning: "Port", secret: false },
                Export { name: "FORGE_PKG_MOSQUITTO_USER", meaning: "User (forge)", secret: false },
                Export { name: "FORGE_PKG_MOSQUITTO_PASSWORD", meaning: "Generated password", secret: true },
            ],
            services: &["mosquitto"],
            paths: &[
                ("/etc/mosquitto/conf.d/90-forge.conf", "listener and auth"),
                ("/etc/mosquitto/forge.passwd", "password file"),
            ],
            used_by: &["Services → package variables"],
            usage: DB_USAGE,
        },
        "apache" => Integration {
            role: "Apache HTTP Server for sites and apps that need it (PHP via mod_php, .htaccess). Put it behind Forge's Nginx edge with a node target.",
            exports: &[Export { name: "FORGE_PKG_APACHE_PORT", meaning: "Port", secret: false }],
            services: &["apache2 / httpd"],
            paths: &[("/etc/apache2", "configuration (Debian)"), ("/etc/httpd", "configuration (RHEL)")],
            used_by: &["Ingress endpoints → node target"],
            usage: SERVICE_USAGE,
        },
        "haproxy" => Integration {
            role: "HAProxy load balancer with a configuration you manage; Forge keeps it installed and updated.",
            exports: &[Export { name: "FORGE_PKG_HAPROXY_CONFIG", meaning: "Configuration file", secret: false }],
            services: &["haproxy"],
            paths: &[("/etc/haproxy/haproxy.cfg", "configuration (yours)")],
            used_by: &["Updates"],
            usage: SERVICE_USAGE,
        },
        "caddy" => Integration {
            role: "Caddy web server with automatic HTTPS, for nodes that serve sites without Forge's Nginx edge.",
            exports: &[
                Export { name: "FORGE_PKG_CADDY_PORT", meaning: "Port of the default site", secret: false },
                Export { name: "FORGE_PKG_CADDY_CONFIG", meaning: "Caddyfile", secret: false },
            ],
            services: &["caddy"],
            paths: &[("/etc/caddy/Caddyfile", "configuration (yours)")],
            used_by: &["Updates"],
            usage: SERVICE_USAGE,
        },
        "tomcat" => Integration {
            role: "Tomcat servlet container for Java web services (WAR files).",
            exports: &[
                Export { name: "FORGE_PKG_TOMCAT_PORT", meaning: "HTTP port", secret: false },
                Export { name: "FORGE_PKG_TOMCAT_WEBAPPS", meaning: "Deploy WAR files here", secret: false },
            ],
            services: &["tomcat10 / tomcat"],
            paths: &[("/etc/tomcat10", "configuration (Debian)"), ("/etc/tomcat", "configuration (RHEL)")],
            used_by: &["Ingress endpoints → node target"],
            usage: SERVICE_USAGE,
        },
        "grafana" => Integration {
            role: "Grafana dashboards; add Prometheus, Loki or your databases as data sources.",
            exports: &[
                Export { name: "FORGE_PKG_GRAFANA_URL", meaning: "URL", secret: true },
                Export { name: "FORGE_PKG_GRAFANA_USER", meaning: "User (admin)", secret: false },
                Export { name: "FORGE_PKG_GRAFANA_PASSWORD", meaning: "Generated admin password", secret: true },
            ],
            services: &["grafana-server"],
            paths: &[("/etc/grafana/grafana.ini", "configuration"), ("/var/lib/grafana", "data")],
            used_by: &["Ingress endpoints → node target"],
            usage: SERVICE_USAGE,
        },
        "loki" => Integration {
            role: "Loki stores logs for Grafana; ship logs to it with Grafana Alloy or Promtail.",
            exports: &[Export { name: "FORGE_PKG_LOKI_URL", meaning: "Push and query URL", secret: true }],
            services: &["loki"],
            paths: &[("/etc/loki/config.yml", "configuration"), ("/var/lib/loki", "data")],
            used_by: &["Grafana data source"],
            usage: SERVICE_USAGE,
        },
        "prometheus" => Integration {
            role: "Prometheus collects metrics; add node exporters and app endpoints as scrape targets.",
            exports: &[
                Export { name: "FORGE_PKG_PROMETHEUS_URL", meaning: "URL", secret: true },
                Export { name: "FORGE_PKG_PROMETHEUS_CONFIG", meaning: "Configuration file", secret: false },
            ],
            services: &["prometheus"],
            paths: &[("/etc/prometheus/prometheus.yml", "scrape configuration"), ("/var/lib/prometheus", "data")],
            used_by: &["Grafana data source"],
            usage: SERVICE_USAGE,
        },
        "node-exporter" => Integration {
            role: "Host metrics (CPU, memory, disks, network) for Prometheus.",
            exports: &[Export { name: "FORGE_PKG_NODE_EXPORTER_URL", meaning: "Metrics URL", secret: true }],
            services: &["prometheus-node-exporter / node_exporter"],
            paths: &[],
            used_by: &["Prometheus scrape target"],
            usage: SERVICE_USAGE,
        },
        "zabbix-agent" => Integration {
            role: "Zabbix agent 2 so an existing Zabbix server monitors this node.",
            exports: &[
                Export { name: "FORGE_PKG_ZABBIX_AGENT_SERVER", meaning: "Zabbix server", secret: false },
                Export { name: "FORGE_PKG_ZABBIX_AGENT_HOSTNAME", meaning: "Host name reported", secret: false },
            ],
            services: &["zabbix-agent2"],
            paths: &[("/etc/zabbix/zabbix_agent2.conf", "configuration")],
            used_by: &["Your Zabbix server"],
            usage: SERVICE_USAGE,
        },
        "jenkins" => Integration {
            role: "Jenkins CI server; finish setup in its web wizard with the one-time password on the node.",
            exports: &[
                Export { name: "FORGE_PKG_JENKINS_URL", meaning: "URL", secret: true },
                Export {
                    name: "FORGE_PKG_JENKINS_INITIAL_ADMIN_PASSWORD_FILE",
                    meaning: "Setup wizard password file",
                    secret: false,
                },
            ],
            services: &["jenkins"],
            paths: &[("/var/lib/jenkins", "home")],
            used_by: &["Ingress endpoints → node target"],
            usage: SERVICE_USAGE,
        },
        "gitlab-ce" => Integration {
            role: "GitLab for Git hosting, CI/CD and a container registry, all in one package.",
            exports: &[
                Export { name: "FORGE_PKG_GITLAB_CE_URL", meaning: "External URL", secret: true },
                Export { name: "FORGE_PKG_GITLAB_CE_USER", meaning: "Admin (root)", secret: false },
                Export { name: "FORGE_PKG_GITLAB_CE_ROOT_PASSWORD", meaning: "Generated root password", secret: true },
            ],
            services: &["gitlab-runsvdir"],
            paths: &[("/etc/gitlab/gitlab.rb", "configuration"), ("/var/opt/gitlab", "data")],
            used_by: &["Ingress endpoints → node target"],
            usage: SERVICE_USAGE,
        },
        _ => return None,
    })
}

#[cfg(test)]
#[allow(clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn every_catalog_package_is_explained() {
        for p in crate::CATALOG {
            let i = integration(p.id).unwrap_or_else(|| panic!("no integration notes for {}", p.id));
            assert!(!i.role.is_empty() && !i.usage.is_empty(), "{}", p.id);
        }
    }
}
