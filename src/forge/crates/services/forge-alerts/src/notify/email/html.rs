//! The HTML part of an alert email (D-119): a branded, table-laid-out page
//! with inline styles only, so Gmail, Outlook and Apple Mail show it the
//! same way. It says the same facts as the plain-text part, adds a
//! plain-language explanation and what to check, and links back to the
//! console.
//!
//! Every dynamic value goes through [`mail_plain`] (one line, bounded,
//! nothing invisible, no bare `scheme://`) and then [`escape`] on the way
//! out; the only `href` is the console's own URL. There are no remote
//! images, no scripts and no tracking: the brand mark is an inline image
//! the transport attaches under [`MARK_CID`].

use std::fmt::Write as _;

use forge_core::alerts::{AlarmState, Severity};
use forge_core::notifications::DeliveryEvent;

use super::super::payload::{AlarmView, Payload};
use super::{mail_plain, observed, utc};

/// The `Content-ID` of the brand mark the transport attaches; the HTML
/// shows it as `cid:` + this.
pub const MARK_CID: &str = "forge-mark@orbyn-forge";

/// Who the message is for: names of the policy and channel that sent it,
/// as they were when the delivery was planned.
#[derive(Debug, Clone, Copy, Default)]
pub struct Why<'a> {
    pub policy: &'a str,
    pub channel: &'a str,
}

// Brand and state colours, from `src/console/src/styles.css` and the brand mark.
const NAVY: &str = "#10314b";
const FG: &str = "#172b45";
const MUTED: &str = "#5f7590";
const LINE: &str = "#d6e4ee";
const PAGE: &str = "#eef5f9";
const ACCENT: &str = "#1f6fc4";
const ACCENT_2: &str = "#3bb8d6";
const RED: &str = "#c63d4f";
const ORANGE: &str = "#d9622b";
const AMBER: &str = "#b06d10";
const GREEN: &str = "#16865f";
const GREY: &str = "#5f7590";
const VIOLET: &str = "#7048c8";
const FONT: &str = "-apple-system,BlinkMacSystemFont,'Segoe UI',Roboto,Helvetica,Arial,sans-serif";

/// Escapes text for HTML element content and quoted attribute values.
#[must_use]
pub fn escape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());

    for c in value.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }

    out
}

/// A dynamic value, made safe for plain text and then for HTML.
fn v(value: &str) -> String {
    escape(&mail_plain(value))
}

fn severity_colour(s: Severity) -> &'static str {
    match s {
        Severity::Critical => RED,
        Severity::Major => ORANGE,
        Severity::Warning => AMBER,
        Severity::Info => ACCENT,
    }
}

/// The banner: its colour and its words.
fn banner(p: &Payload) -> (&'static str, &'static str) {
    let sev = p.alarm.as_ref().map_or(Severity::Info, |a| a.severity);

    match p.event.kind {
        DeliveryEvent::Fired => (severity_colour(sev), "FIRING"),
        DeliveryEvent::Reminder => (severity_colour(sev), "REMINDER \u{b7} STILL FIRING"),
        DeliveryEvent::Resolved => (GREEN, "RESOLVED"),
        DeliveryEvent::Ended => (GREY, "ENDED \u{b7} NO RECOVERY SEEN"),
        DeliveryEvent::Test => (VIOLET, "TEST"),
    }
}

/// A duration in ms as `2 minutes`, `1 hour 5 minutes` or `3 days 4 hours`.
fn span(ms: i64) -> String {
    let mins = ms.max(0) / 60_000;
    let unit = |n: i64, one: &str| if n == 1 { format!("1 {one}") } else { format!("{n} {one}s") };
    let (d, h, m) = (mins / 1440, mins % 1440 / 60, mins % 60);
    match (d, h, m) {
        (0, 0, 0) => "less than a minute".to_owned(),
        (0, 0, m) => unit(m, "minute"),
        (0, h, 0) => unit(h, "hour"),
        (0, h, m) => format!("{} {}", unit(h, "hour"), unit(m, "minute")),
        (d, 0, _) => unit(d, "day"),
        (d, h, _) => format!("{} {}", unit(d, "day"), unit(h, "hour")),
    }
}

/// `2025-11-16 09:30:00 UTC`.
fn when(ms: i64) -> String {
    utc(ms).replace('T', " ").replace('Z', " UTC")
}

/// The resource as a reader names it: `Node lab-b`, `server s-1`.
fn resource_label(p: &Payload) -> String {
    match &p.resource {
        Some(r) => {
            let name = mail_plain(r.name.as_deref().unwrap_or(&r.id));

            let kind = match r.kind.as_str() {
                "server" => "Node".to_owned(),
                "service" => "Service".to_owned(),
                k => mail_plain(k),
            };

            format!("{kind} {name}")
        }
        None => "The resource".to_owned(),
    }
}

/// The observed value as a reader reads it, with `%` for percentages.
fn value_text(p: &Payload) -> Option<String> {
    let o = p.observation.as_ref()?;
    let val = observed(&o.value)?;

    Some(if o.subject.ends_with("percent") { format!("{val}%") } else { val })
}

/// One paragraph saying what happened, and what to check, by subject family.
fn explain(p: &Payload, a: &AlarmView) -> (String, &'static [&'static str]) {
    let who = resource_label(p);
    let subject = p.observation.as_ref().map_or("", |o| o.subject.as_str());
    let value = value_text(p);
    let open = span(p.event.at.0 - a.fired_at.0);
    let closed = a.resolved_at.map(|r| span(r.0 - a.fired_at.0));

    if p.event.kind == DeliveryEvent::Resolved {
        let took = closed.unwrap_or(open);
        return (
            format!(
                "{who} is back to normal. The alarm \u{201c}{}\u{201d} recovered after {took}.",
                mail_plain(&a.title)
            ),
            &[
                "Nothing is needed now. Look at the alarm's history if this keeps happening.",
                "If the cause was a change or an outage, note it for your team.",
            ],
        );
    }

    if p.event.kind == DeliveryEvent::Ended {
        let why = mail_plain(a.end_reason.as_deref().unwrap_or("tracking stopped"));
        return (
            format!(
                "Forge stopped tracking the alarm \u{201c}{}\u{201d} on {} without seeing it recover ({why}). The problem may still be there.",
                mail_plain(&a.title),
                who.to_lowercase_first()
            ),
            &[
                "Check whether the resource still exists or was moved out of the rule's scope.",
                "Check whether the rule was changed or turned off.",
                "Confirm the resource is healthy before you forget about it.",
            ],
        );
    }

    let still = if p.event.kind == DeliveryEvent::Reminder { " still" } else { "" };
    let val = value.as_deref().unwrap_or("unknown");

    if subject == "node.agent.status" {
        return (
            format!(
                "{who} has{still} not reported to Forge for {open}. Its agent status is {val}, so Forge cannot see or manage it."
            ),
            &[
                "Is the machine powered on and reachable on the network?",
                "Is the forge-agent service running on it (systemctl status forge-agent, or Services on Windows)?",
                "Can the node reach the Forge server's address and port (firewall, DNS, proxy)?",
                "Did the agent's certificate expire or get revoked?",
            ],
        );
    }

    let metric = |what: &str| format!("{who} is{still} reporting high {what}: {val}. This started {open} ago.");

    match subject {
        "node.cpu.percent" => (
            metric("CPU use"),
            &[
                "Which processes use the most CPU (top, or Task Manager on Windows)?",
                "Did a deployment, backup or scheduled job start around this time?",
                "Does the node need more CPU, or should load move elsewhere?",
            ],
        ),
        "node.mem.used_percent" => (
            metric("memory use"),
            &[
                "Which processes use the most memory, and is one growing (a leak)?",
                "Is the system swapping, or has the kernel killed processes (out of memory)?",
                "Does the node need more memory, or should load move elsewhere?",
            ],
        ),
        "node.disk.used_percent" => (
            metric("disk use on its system drive"),
            &[
                "What is filling the disk: logs, container images, backups or temporary files?",
                "Can old logs, images or packages be removed (journalctl --vacuum-size, docker system prune)?",
                "Does the volume need to grow before services fail to write?",
            ],
        ),
        s if s.starts_with("health.") => {
            let check = s.rsplit('.').next().unwrap_or(s);
            (
                format!(
                    "The health check \u{201c}{}\u{201d} on {} is{still} failing (observed: {val}). This started {open} ago.",
                    mail_plain(check),
                    who.to_lowercase_first()
                ),
                &[
                    "Open the health check in Forge and read its last result and output.",
                    "Is the service it checks running, and does it answer locally?",
                    "Did a recent deployment or configuration change break it?",
                ],
            )
        }
        s if s.starts_with("app.") => (
            format!("{who} is{still} unhealthy (observed {}: {val}). This started {open} ago.", mail_plain(s)),
            &[
                "Open the service in Forge and look at its instances and recent events.",
                "Read the logs of the failing instance.",
                "Did a recent deployment or change of configuration cause it?",
            ],
        ),
        s if s.starts_with("package.") => (
            format!("{who} has{still} a package problem (observed {}: {val}). This started {open} ago.", mail_plain(s)),
            &[
                "Open the node's packages in Forge and look at the last package job.",
                "Is the package source reachable, and is the package manager locked by another process?",
                "Are security updates waiting to be installed?",
            ],
        ),
        s if s.starts_with("tls.") => (
            format!(
                "A TLS certificate on {} needs attention (observed {}: {val}). This started {open} ago.",
                who.to_lowercase_first(),
                mail_plain(s)
            ),
            &[
                "When does the certificate expire, and is automatic renewal working?",
                "Does the certificate cover the host names clients use?",
                "Is the full chain served, including intermediates?",
            ],
        ),
        s if s.starts_with("node.") => (
            format!("{who} is{still} reporting {}: {val}. This started {open} ago.", mail_plain(s)),
            &[
                "Open the node in Forge and look at its recent metrics and events.",
                "Did anything change on the node around this time?",
            ],
        ),
        _ => {
            let what = if subject.is_empty() { String::new() } else { format!(" ({} = {val})", mail_plain(subject)) };
            (
                format!(
                    "A rule's condition is{still} met on {}{what}. This started {open} ago.",
                    who.to_lowercase_first()
                ),
                &[
                    "Open the alarm in Forge to see the rule and the value that fired it.",
                    "Check the resource for recent changes.",
                ],
            )
        }
    }
}

trait LowerFirst {
    fn to_lowercase_first(&self) -> String;
}

impl LowerFirst for String {
    /// `Node lab-b` reads `node lab-b` inside a sentence.
    fn to_lowercase_first(&self) -> String {
        let mut c = self.chars();
        c.next().map_or_else(String::new, |f| f.to_lowercase().chain(c).collect())
    }
}

/// The console link, when it is an absolute `http(s)` URL (the console has
/// a public URL). Escaped for an attribute.
fn console_link(p: &Payload) -> Option<&str> {
    let l = p.links.as_ref()?.alarm.as_str();
    (l.starts_with("https://") || l.starts_with("http://")).then_some(l)
}

fn row(out: &mut String, label: &str, value_html: &str) {
    let _ = write!(
        out,
        "<tr><td style=\"padding:8px 12px 8px 0;border-bottom:1px solid {LINE};color:{MUTED};font-size:13px;width:130px;vertical-align:top;\">{label}</td>\
         <td style=\"padding:8px 0;border-bottom:1px solid {LINE};color:{FG};font-size:14px;vertical-align:top;word-break:break-word;\">{value_html}</td></tr>"
    );
}

fn details(out: &mut String, p: &Payload, a: &AlarmView) {
    out.push_str("<table role=\"presentation\" width=\"100%\" cellpadding=\"0\" cellspacing=\"0\" border=\"0\" style=\"border-collapse:collapse;\">");
    let sev = a.severity.as_str();
    row(
        out,
        "Severity",
        &format!(
            "<span style=\"display:inline-block;padding:2px 8px;border-radius:10px;background:{};color:#ffffff;font-size:12px;font-weight:600;\">{sev}</span>",
            severity_colour(a.severity)
        ),
    );

    let state = match a.state {
        AlarmState::Firing => "Firing",
        AlarmState::Resolved => "Resolved",
        AlarmState::Ended => "Ended (tracking stopped)",
    };

    let ack = if a.acknowledged { " \u{b7} acknowledged" } else { " \u{b7} not acknowledged" };
    row(out, "State", &format!("{state}{}", if a.state == AlarmState::Firing { ack } else { "" }));

    if let Some(r) = &p.resource {
        row(
            out,
            "Resource",
            &format!(
                "<strong>{}</strong> <span style=\"color:{MUTED};\">({})</span>",
                v(r.name.as_deref().unwrap_or(&r.id)),
                v(&r.kind)
            ),
        );

        if let Some(node) = r.server_id.filter(|n| n.to_string() != r.id) {
            row(out, "Node", &v(&node.to_string()));
        }
    }

    if let Some(o) = &p.observation {
        let val = value_text(p).map(|x| format!(" = <strong>{}</strong>", escape(&x))).unwrap_or_default();
        row(
            out,
            "Observed",
            &format!(
                "<code style=\"font-family:Consolas,Menlo,monospace;font-size:13px;\">{}</code>{val}",
                v(&o.subject)
            ),
        );
    }

    if let Some(rule) = &p.rule {
        row(
            out,
            "Rule",
            &format!("{} <span style=\"color:{MUTED};\">(revision {})</span>", v(&rule.id.to_string()), rule.revision),
        );
    }

    let now = p.event.at.0;
    row(
        out,
        "Fired at",
        &format!("{} <span style=\"color:{MUTED};\">({} ago)</span>", when(a.fired_at.0), span(now - a.fired_at.0)),
    );

    if let Some(at) = a.resolved_at {
        row(
            out,
            "Closed at",
            &format!("{} <span style=\"color:{MUTED};\">(after {})</span>", when(at.0), span(at.0 - a.fired_at.0)),
        );
    }

    out.push_str("</table>");
}

fn head(out: &mut String, title: &str, preheader: &str) {
    let _ = write!(
        out,
        "<!DOCTYPE html><html lang=\"en\"><head><meta charset=\"utf-8\">\
         <meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">\
         <meta name=\"color-scheme\" content=\"light\"><meta name=\"supported-color-schemes\" content=\"light\">\
         <title>{title}</title></head>\
         <body style=\"margin:0;padding:0;background:{PAGE};\">\
         <div style=\"display:none;max-height:0;overflow:hidden;opacity:0;\">{preheader}</div>\
         <table role=\"presentation\" width=\"100%\" cellpadding=\"0\" cellspacing=\"0\" border=\"0\" style=\"background:{PAGE};\"><tr><td align=\"center\" style=\"padding:24px 12px;\">\
         <table role=\"presentation\" width=\"600\" cellpadding=\"0\" cellspacing=\"0\" border=\"0\" style=\"width:100%;max-width:600px;background:#ffffff;border:1px solid {LINE};border-radius:8px;font-family:{FONT};color:{FG};\">\
         <tr><td style=\"background:{NAVY};padding:16px 24px;border-radius:8px 8px 0 0;\">\
         <table role=\"presentation\" cellpadding=\"0\" cellspacing=\"0\" border=\"0\"><tr>\
         <td style=\"vertical-align:middle;padding-right:10px;\"><img src=\"cid:{MARK_CID}\" width=\"32\" height=\"32\" alt=\"\" style=\"display:block;border:0;\"></td>\
         <td style=\"vertical-align:middle;font-family:{FONT};font-size:18px;font-weight:600;color:#ffffff;letter-spacing:0.2px;\">Orbyn <span style=\"color:{ACCENT_2};\">Forge</span></td>\
         </tr></table></td></tr>"
    );
}

fn state_banner(out: &mut String, colour: &str, words: &str, note: Option<&str>) {
    let _ = write!(
        out,
        "<tr><td style=\"background:{colour};padding:10px 24px;color:#ffffff;font-family:{FONT};font-size:13px;font-weight:700;letter-spacing:1px;\">{words}"
    );

    if let Some(n) = note {
        let _ = write!(out, "<span style=\"font-weight:400;letter-spacing:0;\"> \u{b7} {n}</span>");
    }

    out.push_str("</td></tr>");
}

fn button(out: &mut String, href: &str, label: &str) {
    let _ = write!(
        out,
        "<table role=\"presentation\" cellpadding=\"0\" cellspacing=\"0\" border=\"0\" style=\"margin:24px 0 8px;\"><tr>\
         <td style=\"background:{ACCENT};border-radius:6px;\"><a href=\"{}\" style=\"display:inline-block;padding:12px 22px;font-family:{FONT};font-size:15px;font-weight:600;color:#ffffff;text-decoration:none;\">{label}</a></td>\
         </tr></table>",
        escape(href)
    );
}

fn foot(out: &mut String, p: &Payload, why: Why<'_>, alarm: Option<&AlarmView>) {
    let mut reason = String::from("You get this email because ");

    match (why.policy.trim(), why.channel.trim()) {
        ("", "") => reason.push_str("an Orbyn Forge notification channel sends to this address."),
        ("", c) => {
            let _ = write!(reason, "the notification channel \u{201c}{}\u{201d} sends to this address.", v(c));
        }
        (pol, "") => {
            let _ = write!(reason, "the notification policy \u{201c}{}\u{201d} includes you.", v(pol));
        }
        (pol, c) => {
            let _ = write!(
                reason,
                "the notification policy \u{201c}{}\u{201d} sends alarms like this one through the channel \u{201c}{}\u{201d}.",
                v(pol),
                v(c)
            );
        }
    }

    reason
        .push_str(" To change who gets these messages, edit the policy in <strong>Alerts \u{2192} Policies</strong>.");

    let ids = match alarm {
        Some(a) => format!("Alarm {} \u{b7} delivery {}", v(&a.id.to_string()), v(&p.delivery_id.to_string())),
        None => format!("Delivery {}", v(&p.delivery_id.to_string())),
    };

    let _ = write!(
        out,
        "<tr><td style=\"padding:16px 24px 20px;border-top:1px solid {LINE};font-family:{FONT};font-size:12px;line-height:18px;color:{MUTED};\">\
         <p style=\"margin:0 0 8px;\">{reason}</p>\
         <p style=\"margin:0;\"><strong style=\"color:{FG};\">Orbyn Forge</strong> \u{b7} {ids}</p>\
         </td></tr></table></td></tr></table></body></html>"
    );
}

/// The HTML body.
#[must_use]
pub fn html(p: &Payload, why: Why<'_>) -> String {
    let mut out = String::with_capacity(8 * 1024);
    let (colour, words) = banner(p);
    let note = p.after_silence.is_some().then_some("sent after a silence ended");

    let Some(a) = &p.alarm else {
        let msg = v(p.message.as_deref().unwrap_or("Test notification from Orbyn Forge."));
        head(&mut out, "Test notification from Orbyn Forge", &msg);
        state_banner(&mut out, colour, words, None);
        let _ = write!(
            out,
            "<tr><td style=\"padding:24px;font-family:{FONT};\">\
             <h1 style=\"margin:0 0 12px;font-size:20px;line-height:26px;color:{FG};\">Test notification</h1>\
             <p style=\"margin:0;font-size:15px;line-height:22px;color:{FG};\">{msg} This channel can deliver Orbyn Forge alarm notifications, and this is how they look.</p>\
             </td></tr>"
        );
        foot(&mut out, p, why, None);

        return out;
    };

    let (explanation, checks) = explain(p, a);
    let title = v(&a.title);
    head(&mut out, &title, &escape(&explanation));
    state_banner(&mut out, colour, words, note);
    let _ = write!(
        out,
        "<tr><td style=\"padding:24px 24px 8px;font-family:{FONT};\">\
         <h1 style=\"margin:0 0 12px;font-size:21px;line-height:28px;color:{FG};\">{title}</h1>\
         <p style=\"margin:0 0 16px;font-size:15px;line-height:22px;color:{FG};\">{}</p>",
        escape(&explanation)
    );

    if p.event.kind == DeliveryEvent::Reminder && !a.acknowledged {
        let _ = write!(
            out,
            "<p style=\"margin:0 0 16px;padding:10px 12px;background:#fbf1df;border-left:3px solid {AMBER};font-size:14px;color:{FG};\">Nobody has acknowledged this alarm yet.</p>"
        );
    }

    if p.after_silence.is_some() {
        let _ = write!(
            out,
            "<p style=\"margin:0 0 16px;padding:10px 12px;background:{PAGE};border-left:3px solid {ACCENT};font-size:14px;color:{FG};\">A silence held this message back. It is sent now because that silence has ended.</p>"
        );
    }

    let _ = write!(
        out,
        "<h2 style=\"margin:8px 0 6px;font-size:14px;text-transform:uppercase;letter-spacing:0.6px;color:{MUTED};\">What to check</h2>\
         <ul style=\"margin:0 0 20px;padding-left:20px;font-size:14px;line-height:21px;color:{FG};\">"
    );

    for c in checks {
        let _ = write!(out, "<li style=\"margin:0 0 4px;\">{}</li>", escape(c));
    }

    out.push_str("</ul>");
    details(&mut out, p, a);

    if let Some(link) = console_link(p) {
        button(&mut out, link, "Open alarm in Forge");
        let silences = link.split_once("/alerts?").map(|(base, _)| format!("{base}/alerts?tab=silences"));
        let _ = write!(
            out,
            "<p style=\"margin:0 0 8px;font-size:13px;line-height:20px;color:{MUTED};\">On the alarm page you can <a href=\"{}\" style=\"color:{ACCENT};\">acknowledge it</a> so your team knows someone is on it, or <a href=\"{}\" style=\"color:{ACCENT};\">silence it</a> while you work.",
            escape(link),
            escape(link)
        );

        if let Some(s) = silences {
            let _ = write!(out, " <a href=\"{}\" style=\"color:{ACCENT};\">See all silences</a>.", escape(&s));
        }

        out.push_str("</p>");
    }

    out.push_str("</td></tr>");
    foot(&mut out, p, why, Some(a));

    out
}

#[cfg(test)]
mod tests;
