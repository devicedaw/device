# Security policy

## Supported versions

Device is pre-alpha. Only the latest release and the `main` branch are supported, and fixes land on `main`.

## Reporting a vulnerability

Please do not report security problems in public issues, discussions or pull requests.

Report them privately through GitHub instead: open the repository's **Security** tab and choose **Report a
vulnerability** ([direct link](https://github.com/devicedaw/device/security/advisories/new)). Include:

- what the problem is and what an attacker could do with it,
- steps or a file (for example a crafted project, audio file or plugin) that reproduces it,
- the system and the commit you tested.

You should get a reply within a week. Once a fix is merged we will publish an advisory and credit you, unless you would
rather stay anonymous.

## Scope

Things we especially want to hear about:

- crashes or memory errors when opening a project, audio file or MIDI file someone else made,
- the plugin scanner or host running code it shouldn't, or a plugin escaping its out-of-process scan,
- the optional Assistant leaking your API key or sending project data you didn't ask it to send.

Third-party plugins themselves are out of scope: report problems in a plugin to its vendor.
