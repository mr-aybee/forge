Orbyn Forge @VERSION@
=====================

Everything needed to run the Forge control plane is in this folder:

  forge-server            control plane (web console built in)
  forge-analytics-worker  analytics process, started and supervised by forge-server
  forge-plugin-runner     sandbox for plugins, started by forge-server
  dist/                   agent binaries handed to your nodes, with
                          SHA256SUMS and MANIFEST.json (their version)
  install.sh / .ps1       installer
  forge.toml.example      configuration reference

Linux (systemd), as root:

  sudo ./install.sh                       # asks a few questions
  sudo ./install.sh --yes --public-url http://forge.example.com:8080

Windows (PowerShell as Administrator):

  .\install.ps1 -PublicUrl http://forge.example.com:8080

The installer checks this folder against dist/SHA256SUMS before it changes
anything. To check the download itself, verify SHA256SUMS.sig from the
release (get-forge.sh --verify-only does this).

The installer creates the administrator "admin" with a generated password,
shows it once and saves it (root/Administrators only) until you change it.
Then open the public URL and sign in. Nodes need TCP 7443 and 55132 to this
server. Updates: the console shows "Update available" at the top right.

Uninstall: sudo sh /etc/forge/install.sh --uninstall   (Windows: .\install.ps1 -Uninstall)

Guide: https://forge.orbyntech.com/docs/getting-started/installation/
