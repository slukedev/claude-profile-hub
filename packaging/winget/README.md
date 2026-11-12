# winget

The first version goes to [microsoft/winget-pkgs](https://github.com/microsoft/winget-pkgs) by
hand. After it's merged, the Release workflow opens update PRs on its own (it needs a
`WINGET_TOKEN` secret: a classic GitHub token with `public_repo` scope).

For the first submission, after the v0.1.0 release is published:

1. Copy the two `.zip` hashes from the release's `SHA256SUMS` into
   `slukedev.ClaudeProfileHub.installer.yaml`.
2. Check the manifests: `winget validate --manifest packaging/winget`
3. Test locally: `winget install --manifest packaging/winget`
4. Fork winget-pkgs, add the three files under
   `manifests/s/slukedev/ClaudeProfileHub/0.1.0/`, and open a PR.

Or let `wingetcreate` do steps 1 to 4:

```powershell
wingetcreate new https://github.com/slukedev/claude-profile-hub/releases/download/v0.1.0/claude-profile-hub-x86_64-pc-windows-msvc.zip https://github.com/slukedev/claude-profile-hub/releases/download/v0.1.0/claude-profile-hub-aarch64-pc-windows-msvc.zip
```
