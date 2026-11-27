// Guardrails: platform-wide `forbid` policies. A forbid always overrides a
// permit, so these hold no matter which roles a user has been granted.

// Sensitive actions require an MFA-verified session (step-up MFA).
@id("guard:sensitive-requires-mfa")
forbid (
  principal,
  action in [
    Forge::Action::"server.command",
    Forge::Action::"process.manage",
    Forge::Action::"network.tunnel",
    Forge::Action::"network.tunnel.any",
    Forge::Action::"node.onboard",
    Forge::Action::"package.manage",
    Forge::Action::"proxy.manage",
    Forge::Action::"terminal.open",
    Forge::Action::"health.command",
    Forge::Action::"secret.read",
    Forge::Action::"access.manage",
    Forge::Action::"secret.manage",
    Forge::Action::"db.admin",
    Forge::Action::"plugin.manage",
    Forge::Action::"system.update",
    Forge::Action::"analytics.configure",
    Forge::Action::"file.write",
    Forge::Action::"file.delete",
    Forge::Action::"file.admin",
    Forge::Action::"stack.privileged",
    Forge::Action::"swarm.manage",
    Forge::Action::"registry.manage",
    Forge::Action::"webhook.manage",
    Forge::Action::"certificate.use",
    Forge::Action::"container.inspect_env",
    Forge::Action::"notification.manage"
  ],
  resource
)
unless { context.mfa };
