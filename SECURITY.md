# Reporting a security issue

Report security concerns using [GitHub private vulnerability reporting](https://github.com/glyphfolio/glyphfolio-editor/security/advisories/new). Do not publish private document content, credentials, or exploit details in a public issue.

Include a minimal synthetic reproducer, exact revision/dependencies, operating system, and the observed effect. All versions are experimental; no released supported-version window is currently declared.

Untrusted Markdown must never execute commands or trigger resource loading merely by opening it. Clipboard conversion, links, future assets, and host integrations require explicit boundaries. Engine fixes need atomicity, Unicode, and regression evidence proportional to the issue.
