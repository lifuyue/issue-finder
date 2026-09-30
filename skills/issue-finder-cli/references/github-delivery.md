# GitHub delivery in a managed environment

Use this reference when the requested endpoint includes a fork, push, or PR, or
when diagnosing GitHub authentication. User authorization, credential selection,
resource access, and successful execution are different facts. Reuse authorization
already given; do not ask the user to approve the same operation again to fix a 403.

## Check the actual operations early

Record the source repository, its real default branch, the intended fork owner,
and the requested endpoint. Check the following independently:

| Capability | Useful evidence | Does not establish |
| --- | --- | --- |
| CLI installation | Installed executable on PATH and compatible session catalog | Future cloud tasks have the installation |
| GitHub reads | Successful status/issue/repository request | Fork, push, or PR write access |
| Fork | Existing fork with the correct parent, or successful authorized creation | Upstream PR permission |
| Git push | Intended branch and commit exist on the remote after push | API identity or PR creation permission |
| Upstream PR | Successful creation/read-back of the actual upstream PR | CI success or maintainer acceptance |

Use read-only evidence first. Never create throwaway PRs or repositories merely
to probe permissions. When the full contribution is already authorized, creating
the needed fork early is useful preparation. There is no universal read-only
check that guarantees a later PR mutation will succeed: label untested operations
as unknown. A read-only score or `permissions.push` on one's fork is not proof of
upstream PR write access. Check that an existing fork belongs to the same network.

## Select a suitable credential, then verify its use

A securely supplied token can remove interactive setup, provided both its GitHub
permissions/resource coverage and the environment's credential routing support
the requested operations. Do not recommend an unrestricted token by default.

- Use environment Secrets or the supported identity mechanism, not chat, source
  files, PR bodies, or logs. Request only permissions needed for the actual scope.
- GitHub App credentials are bounded by the app's permissions and installation
  access. Check suspended installations and repository coverage when evidence
  points there; network access does not grant GitHub authorization.
- Fine-grained PATs have owner/repository restrictions. GitHub currently documents
  limitations for contributing to public repositories outside the user's ownership
  or organization membership. A PAT scoped only to the fork must not be advertised
  as sufficient for all upstream PR operations. Consult the current docs below.
- For public cross-owner contributions, a user OAuth credential or, where needed
  and allowed, a classic PAT with `public_repo` may fit. Private repositories,
  workflow changes, SSO, and organization policies need separate consideration;
  do not request broad `repo`, workflow, or administration access without a reason.
- `gh` prefers `GH_TOKEN` over `GITHUB_TOKEN` and stored login credentials. Issue
  Finder session tools resolve `GITHUB_TOKEN`, configured token, then captured
  `gh auth token`. These clients can select different identities. Use safe status
  output and source labels; never print token values or entire credential files.

GitHub documents these fine-grained endpoint permissions; they do not override
resource-owner restrictions or guarantee cross-owner coverage:

| Operation | Endpoint permission |
| --- | --- |
| Create a fork | Administration: write and Contents: read |
| Create a pull request | Pull requests: write on the target resource |

Git push separately needs repository content write access; workflow changes may
require additional permissions. Prefer an identity suited to the contribution
over telling the user to obtain administration rights on an upstream they do not own.

Official references:

- [gh environment precedence](https://cli.github.com/manual/gh_help_environment)
- [PAT types and limitations](https://docs.github.com/en/authentication/keeping-your-account-and-data-secure/managing-your-personal-access-tokens)
- [Create a fork](https://docs.github.com/en/rest/repos/forks#create-a-fork)
- [Create a PR](https://docs.github.com/en/rest/pulls/pulls#create-a-pull-request)

## Diagnose once instead of looping through authorization

1. Identify the failed operation, repository, HTTP status, and safe error message.
   A secondary rate-limit 403, suspended installation, missing permission, and
   Git credential-helper failure require different remedies.
2. Inspect the selected client's identity and credential source. A successful
   `gh auth login` does not establish that subsequent commands use that login.
   Refreshing stored OAuth scopes cannot expand an injected GitHub App token.
3. In managed clouds, preserve the configured proxy, CA, and injected identity.
   Follow the host's supported configuration workflow. If explicitly authorized
   interactive login is appropriate, use an isolated CLI configuration and only
   the host-supported credential selection mechanism; do not overwrite global
   credentials or bypass enforced authentication/network routing.
4. Check actual Git and API behavior separately after a credential change. If
   push works but PR creation still reports `Resource not accessible by integration`,
   report that distinction. Do not promise that another device-code login fixes it.
   Empty OAuth scope headers alone are not proof of missing permission: App and
   fine-grained credentials can lack classic scope headers. Authenticated API
   responses without an explicit Authorization header demonstrate automatic
   authentication, but do not by themselves prove how every supplied token is routed.
5. Retry only after a relevant change or with a genuinely different supported
   credential path. Trying REST after GraphQL through the same denied identity
   is not a permission fix. If routing/configuration cannot be changed here, say
   so, keep the prepared work, and give the smallest remaining action (for example,
   creating the missing fork or submitting the prepared PR in GitHub's UI).

During one observed cloud contribution, device login succeeded and its credential
could push the fork, while API PR creation remained forbidden as an integration.
This is evidence of distinct effective Git/API capabilities, not evidence that
all Codex Cloud environments always ignore user tokens. Do not prescribe repeated
OAuth authorization or guarantee that injecting a PAT alone solves that case.

## Review and validation

- Read applicable contribution instructions, PR templates, nearby code/tests, and
  current default-branch behavior. Record the base commit. Separate actual
  reproduction from an issue author's claim and from static inference.
- Judge total diff and maintenance cost, not only the production file. Keep the
  fix narrow; avoid copying another SDK's behavior when compatibility is unresolved.
  Do not add retries, broad catches, generic validation, or new test-injection APIs
  without a concrete need.
- Decide from the user's instructions and repository requirements whether new
  tests belong in the PR. Temporary probes can establish before/after behavior
  without being committed. Reuse existing tests where practical; large subprocess,
  reflection, mock-server, and environment-management scaffolds require justification.
  Honor a code-only submission request and disclose any resulting unmet repository
  expectation instead of silently adding tests anyway.
- Prefer a failing baseline and a passing fix where practical. Record actual
  commands/results and whether dependencies, transport, or credentials were mocked.
  An isolated build of selected sources is not the full repository suite; say so.
- Keep probes, logs, credentials, local build configuration, and task metadata out
  of commits. Settle submission scope before committing/pushing to avoid publishing
  temporary test scaffolding and then removing it. A later deletion removes the
  final diff, not earlier commits; do not rewrite shared history without authorization.
- Re-run checks when changes or failures justify them, not merely for reassurance.
  Removing a temporary test from the submission does not invalidate already-recorded
  execution evidence for unchanged production behavior. Update test counts and
  submitted-file claims in the PR description nonetheless.

## Publish and confirm

1. Review the final base-to-head diff, including committed files and history as
   relevant, then commit only the authorized contribution. Respect target style.
2. Push to the confirmed writable remote. Check the exit status and remote branch
   SHA; an `Everything up-to-date` line following an authentication error is not
   successful delivery. Use an existing compatible Git credential helper or an
   authorized command-scoped helper without putting tokens in URLs or arguments.
3. Create the PR against the upstream's actual default branch with an explicit
   fork owner/head. Use a structured tool argument or `gh pr create --body-file`
   with actual newlines. Check for an existing PR for that head before retrying
   an uncertain creation result.
4. Use the requested language unless repository contribution requirements say
   otherwise. Explain the concrete bug, resulting behavior, relevant validation,
   and limits. Use a related-issue reference instead of an auto-closing keyword
   when the patch resolves only part of the issue. Rebuild the title/body around
   the final scope after trimming tests or changing the approach.
5. Read back the PR URL, base/head, commit and changed files. Attach it using the
   host's artifact tool where available. Report CI as pending/not checked unless
   observed; local checks and PR creation do not establish CI success or merge.

If creation remains blocked, provide the pushed branch/commit, a short working
GitHub compare link, and copyable title AND description together. Long URL-encoded
descriptions can fail in mobile/app navigation; a bare compare link will not fill
the description. Never claim the PR exists until a real PR number/URL is returned
or read back after the user submits it. Continue automatically when the user
reports the required external step is complete; do not request publishing consent again.
