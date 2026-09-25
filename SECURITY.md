# Security policy

## Reporting a vulnerability

Please don't open a public issue. Use GitHub's private reporting instead:
**Security → Report a vulnerability** on this repository. If that isn't
possible, email the maintainers listed in [MAINTAINERS.md](MAINTAINERS.md).

Include what you found, how to reproduce it, and which version or commit you
tested. You should get an acknowledgement within a few days, and we'll agree a
disclosure timeline with you before anything is published.

## Scope

In scope: anything that could make the engine send orders it shouldn't, bypass
risk checks or the kill switch, leak credentials (API keys read from config or
the environment), or let a remote party control a running instance.

The web console listens on `127.0.0.1` by default and has **no authentication**.
Binding it to a public interface (`http.listen = "0.0.0.0:8080"`) exposes the
kill switch and session data to anyone who can reach the port. Put it behind a
reverse proxy with authentication, or an SSH tunnel, if you need remote access.

## Supported versions

Until 1.0, only the latest release gets security fixes.
