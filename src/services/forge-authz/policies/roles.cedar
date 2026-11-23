// Role templates. One template per built-in role. A role binding links a
// template with ?principal = the user and ?resource = the scope (Platform,
// Workspace, Tenant, Environment or Cluster). `resource in ?resource` makes the
// grant cover the scope itself and everything beneath it. `principal in
// ?principal` is the user itself, or a group the user is a member of (#13).

@id("role:platform_admin")
permit (principal in ?principal, action in Forge::Action::"role:platform_admin", resource in ?resource);

@id("role:workspace_admin")
permit (principal in ?principal, action in Forge::Action::"role:workspace_admin", resource in ?resource);

@id("role:operator")
permit (principal in ?principal, action in Forge::Action::"role:operator", resource in ?resource);

@id("role:developer")
permit (principal in ?principal, action in Forge::Action::"role:developer", resource in ?resource);

@id("role:dba")
permit (principal in ?principal, action in Forge::Action::"role:dba", resource in ?resource);

@id("role:auditor")
permit (principal in ?principal, action in Forge::Action::"role:auditor", resource in ?resource);

@id("role:customer")
permit (principal in ?principal, action in Forge::Action::"role:customer", resource in ?resource);
