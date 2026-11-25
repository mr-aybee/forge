//! Compose app templates (D-160): ready compose stacks with pinned images,
//! health checks and named volumes. A template's `${VAR}` placeholders are
//! filled from the stack's `.env` (Docker Compose interpolation); secret
//! variables are generated when left empty and stored sealed like any
//! stack `.env`.

use serde::Serialize;

/// One variable a template asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Variable {
    pub name: &'static str,
    pub label: &'static str,
    pub default: &'static str,
    /// A password or key: generated when left empty, never shown again.
    pub secret: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Template {
    pub id: &'static str,
    pub title: &'static str,
    pub description: &'static str,
    pub category: &'static str,
    pub compose: &'static str,
    pub variables: &'static [Variable],
}

const fn var(name: &'static str, label: &'static str, default: &'static str) -> Variable {
    Variable { name, label, default, secret: false }
}

const fn secret(name: &'static str, label: &'static str) -> Variable {
    Variable { name, label, default: "", secret: true }
}

pub const TEMPLATES: &[Template] = &[
    Template {
        id: "nginx",
        title: "Nginx",
        description: "A static web server serving the `site` volume.",
        category: "Web",
        compose: r#"services:
  web:
    image: nginx:1.27-alpine
    restart: unless-stopped
    ports:
      - "${PORT}:80"
    volumes:
      - site:/usr/share/nginx/html:ro
    healthcheck:
      test: ["CMD", "wget", "-q", "--spider", "http://localhost/"]
      interval: 30s
volumes:
  site:
"#,
        variables: &[var("PORT", "Published port", "8080")],
    },
    Template {
        id: "postgres",
        title: "PostgreSQL",
        description: "PostgreSQL 17 with a persistent data volume.",
        category: "Database",
        compose: r#"services:
  db:
    image: postgres:17-alpine
    restart: unless-stopped
    environment:
      POSTGRES_USER: ${POSTGRES_USER}
      POSTGRES_PASSWORD: ${POSTGRES_PASSWORD}
      POSTGRES_DB: ${POSTGRES_DB}
    ports:
      - "${PORT}:5432"
    volumes:
      - data:/var/lib/postgresql/data
    healthcheck:
      test: ["CMD-SHELL", "pg_isready -U ${POSTGRES_USER}"]
      interval: 10s
volumes:
  data:
"#,
        variables: &[
            var("POSTGRES_USER", "User", "app"),
            secret("POSTGRES_PASSWORD", "Password"),
            var("POSTGRES_DB", "Database", "app"),
            var("PORT", "Published port", "5432"),
        ],
    },
    Template {
        id: "redis",
        title: "Redis",
        description: "Redis 7 with append-only persistence and a password.",
        category: "Database",
        compose: r#"services:
  cache:
    image: redis:7-alpine
    restart: unless-stopped
    command: ["redis-server", "--appendonly", "yes", "--requirepass", "${REDIS_PASSWORD}"]
    ports:
      - "${PORT}:6379"
    volumes:
      - data:/data
    healthcheck:
      test: ["CMD-SHELL", "redis-cli -a \"$$REDIS_PASSWORD\" ping"]
      interval: 10s
    environment:
      REDIS_PASSWORD: ${REDIS_PASSWORD}
volumes:
  data:
"#,
        variables: &[secret("REDIS_PASSWORD", "Password"), var("PORT", "Published port", "6379")],
    },
    Template {
        id: "mariadb",
        title: "MariaDB",
        description: "MariaDB 11 with a persistent data volume.",
        category: "Database",
        compose: r#"services:
  db:
    image: mariadb:11
    restart: unless-stopped
    environment:
      MARIADB_ROOT_PASSWORD: ${ROOT_PASSWORD}
      MARIADB_DATABASE: ${DATABASE}
      MARIADB_USER: ${USER}
      MARIADB_PASSWORD: ${PASSWORD}
    ports:
      - "${PORT}:3306"
    volumes:
      - data:/var/lib/mysql
    healthcheck:
      test: ["CMD", "healthcheck.sh", "--connect", "--innodb_initialized"]
      interval: 10s
volumes:
  data:
"#,
        variables: &[
            secret("ROOT_PASSWORD", "Root password"),
            var("DATABASE", "Database", "app"),
            var("USER", "User", "app"),
            secret("PASSWORD", "Password"),
            var("PORT", "Published port", "3306"),
        ],
    },
    Template {
        id: "wordpress",
        title: "WordPress",
        description: "WordPress with its own MariaDB, both on persistent volumes.",
        category: "Web",
        compose: r#"services:
  wordpress:
    image: wordpress:6-apache
    restart: unless-stopped
    depends_on:
      db:
        condition: service_healthy
    environment:
      WORDPRESS_DB_HOST: db
      WORDPRESS_DB_USER: wordpress
      WORDPRESS_DB_PASSWORD: ${DB_PASSWORD}
      WORDPRESS_DB_NAME: wordpress
    ports:
      - "${PORT}:80"
    volumes:
      - site:/var/www/html
  db:
    image: mariadb:11
    restart: unless-stopped
    environment:
      MARIADB_ROOT_PASSWORD: ${DB_ROOT_PASSWORD}
      MARIADB_DATABASE: wordpress
      MARIADB_USER: wordpress
      MARIADB_PASSWORD: ${DB_PASSWORD}
    volumes:
      - db:/var/lib/mysql
    healthcheck:
      test: ["CMD", "healthcheck.sh", "--connect", "--innodb_initialized"]
      interval: 10s
volumes:
  site:
  db:
"#,
        variables: &[
            secret("DB_PASSWORD", "Database password"),
            secret("DB_ROOT_PASSWORD", "Database root password"),
            var("PORT", "Published port", "8080"),
        ],
    },
    Template {
        id: "uptime-kuma",
        title: "Uptime Kuma",
        description: "A self-hosted uptime monitor.",
        category: "Monitoring",
        compose: r#"services:
  kuma:
    image: louislam/uptime-kuma:1
    restart: unless-stopped
    ports:
      - "${PORT}:3001"
    volumes:
      - data:/app/data
volumes:
  data:
"#,
        variables: &[var("PORT", "Published port", "3001")],
    },
];

#[must_use]
pub fn find(id: &str) -> Option<&'static Template> {
    TEMPLATES.iter().find(|t| t.id == id)
}

/// A value for a `.env` line: printable, one line, bounded, no `'` (it is
/// written single-quoted, and the `.env` reader does not unescape).
fn value_ok(v: &str) -> bool {
    v.len() <= 1024 && !v.chars().any(|c| c.is_control() || c == '\'')
}

/// The stack's `.env` for `values` (by variable name). Unknown names and
/// bad values are refused; empty secrets are generated by `generate`.
pub fn render_env(
    t: &Template,
    values: &std::collections::BTreeMap<String, String>,
    mut generate: impl FnMut() -> String,
) -> Result<String, String> {
    if let Some(k) = values.keys().find(|k| !t.variables.iter().any(|v| v.name == k.as_str())) {
        return Err(format!("`{k}` is not a variable of the {} template", t.title));
    }

    let mut out = String::new();

    for v in t.variables {
        let given = values.get(v.name).map(|s| s.trim()).filter(|s| !s.is_empty());
        let value = match given {
            Some(s) => s.to_owned(),
            None if v.secret => generate(),
            None => v.default.to_owned(),
        };

        if !value_ok(&value) {
            return Err(format!("`{}`: one printable line without `'`, at most 1024 characters", v.name));
        }

        // Single-quoted, so `#` and spaces stay part of the value.
        out.push_str(&format!("{}='{}'\n", v.name, value));
    }

    Ok(out)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn every_template_declares_its_placeholders_and_pins_images() {
        for t in TEMPLATES {
            for line in t.compose.lines().filter(|l| l.trim_start().starts_with("image:")) {
                assert!(line.contains(':') && !line.trim_end().ends_with(":latest"), "{}: {line}", t.id);
            }

            let mut rest = t.compose;

            while let Some(i) = rest.find("${") {
                let end = rest[i..].find('}').unwrap();
                let name = &rest[i + 2..i + end];
                assert!(t.variables.iter().any(|v| v.name == name), "{}: ${{{name}}} is not declared", t.id);
                rest = &rest[i + end..];
            }

            assert!(find(t.id).is_some());
        }
    }

    #[test]
    fn env_is_rendered_with_generated_secrets() {
        let t = find("postgres").unwrap();
        let mut values = BTreeMap::new();
        values.insert("POSTGRES_DB".to_owned(), "shop".to_owned());
        let env = render_env(t, &values, || "generated".into()).unwrap();
        assert!(env.contains("POSTGRES_DB='shop'"));
        assert!(env.contains("POSTGRES_PASSWORD='generated'"));
        assert!(env.contains("POSTGRES_USER='app'"));
        values.insert("EVIL".into(), "x".into());
        assert!(render_env(t, &values, String::new).is_err());
        let mut bad = BTreeMap::new();
        bad.insert("POSTGRES_DB".to_owned(), "a\nb=c".to_owned());
        assert!(render_env(t, &bad, String::new).is_err());
    }
}
