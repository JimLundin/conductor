# Project backend configuration

A project's `project.toml` selects version control, issue tracking and
repository hosting independently. Omitting a selection configures no
backend for that work. The project's existing `remote` still describes
where its checkout can be cloned; it does not select an issue tracker or
hosting service.

```toml
[backends.version_control]
adapter = "git2"
resource = "/projects/local"

[backends.tracker]
adapter = "github"
resource = "team/issues"
credential = "host/github-issues"
options = '''
namespace = "work/"
'''

[backends.hosting]
adapter = "github"
resource = "team/builds"
credential = "host/github-builds"
```

`credential` names a secret in host storage. It contains no token.
Credential references cannot traverse directories, and backend resources
reject URLs with embedded credentials or query strings. `options` is TOML
text interpreted by the selected adapter. GitHub mapping options are
validated against the loaded flow by the pure `github` crate. Its defaults
and native relation choices do not apply to other adapters.

Run `home project capabilities <project>` from the home checkout to
validate the configuration and inspect the available operations. This
reads the project catalog without refreshing issue records. Tracker mapping
validation also reads the issue flow. Local and hosting capability inspection
works when that flow is missing or invalid and no tracker needs it.
Malformed project configuration is still found and reported.
Invalid configuration fails the query. A successful query can report
unavailable operations, including missing backends, missing or expired
credentials, and unsupported capabilities.

The query currently reports configured adapters as unavailable until the
binary registers constructed instances. The registry contains no default
or placeholder implementation. The adapter and credential implementation
tasks will supply those instances.

`project-runtime::Registry` provides the same registration, capability and
preflight query for CLI and daemon construction. A binding includes its
complete selection, resource, credential reference and options. Changing
any of these requires a new binding. Capability checks run before adapter
dispatch and apply to the requested operation. A tracker failure does not
prevent local workspace preparation, snapshots or diffs. Issue work without
a tracker returns `TrackerNotConfigured`.

Repository hosting declares statuses, checks and workflows separately.
Access to statuses does not imply access to required checks. Hosted-result
requests retain their exact revision. Workspace methods use `vcs` values
for preparation, capture, comparison, integration and preservation; Git
commands, indexes and object operations do not cross this interface.
