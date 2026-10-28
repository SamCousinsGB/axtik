# Axtik security

This preview reads project source and starts a native child process. The extension requires workspace trust, passes arguments without a shell, bounds output and deadlines, and uses only an explicit native executable. Local engine execution has no network adapter and does not read Salesforce credentials.

Process isolation is not an OS security sandbox. Run untrusted repositories in an isolated environment. Keep the engine and integrations updated together so protocol validation remains effective.

Report suspected vulnerabilities privately through [GitHub's security advisory form](https://github.com/SamCousinsGB/axtik/security/advisories/new). Include the affected version, a minimal synthetic reproduction and the observed impact. Do not publish secrets or sensitive customer projects as issue attachments. If private reporting is unavailable, open a public issue asking the maintainer to enable it without including vulnerability details.

The first release is an experimental developer preview, not a completed security audit or Salesforce compatibility certification.
