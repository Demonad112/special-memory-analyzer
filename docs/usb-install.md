# USB install kit

Each release publishes `Deep-Server-Analyzer-usb-kit.zip`. It holds the signed setup, the signing
certificate, and scripts that trust that certificate for a limited time, install the app silently,
and remove the trust when the time is up.

Only run this on servers you are authorized to change.

## Signing and keys

The release certificate is ephemeral. The release job creates a self-signed code-signing certificate,
signs the installer and app, then deletes the certificate and its private key from the runner. No
private key is stored in the repo or in secrets, and nothing can re-sign a release. The thumbprint is in
the release notes.

## Before you start

1. Download the kit zip, `dsa-signing.cer` and `SHA256SUMS.txt` from the release.
2. Check the thumbprint in the release notes against the one the script prints.
3. Run `Install-DSA.cmd` only from a folder you extracted yourself.

## Installer window

Extract the kit zip, then double-click `DSA-Installer.cmd` and accept the UAC prompt. The window has:

- **Install**: verifies the kit, trusts the certificate for 8 h, schedules removal and runs the silent setup.
- **▲ +1 h**: adds an hour to the trust window (capped at 72 h from now).
- **✕ Remove trust** (red): opens a PowerShell window that removes the trust, then prints an OK or FAIL line for each store, the `DSA-TrustExpiry` task and the expiry record. The window turns green only when every check passes.

Closing the window keeps the timer running. Reopening `DSA-Installer.cmd` while trust is active goes straight to the countdown. The scripts below still work on their own.

## Files in the kit

| File                                                  | What it does                                                                                                                                                            |
| ----------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `DSA-Installer.cmd` / `.ps1`                          | The installer window described above.                                                                                                                                   |
| `Install-DSA.cmd` / `.ps1`                            | Verifies the kit, asks how long trust should last, trusts the certificate, schedules its removal, checks the setup signature, runs the setup, then shows the countdown. |
| `Trust-Only.cmd`                                      | Trust and schedule removal, no install.                                                                                                                                 |
| `Install-Only.cmd`                                    | Runs the setup only. The certificate must already be trusted.                                                                                                           |
| `Show-Trust-Timer.cmd` / `.ps1`                       | Reattaches to the countdown at any time.                                                                                                                                |
| `Remove-Trust.cmd` / `.ps1`                           | Removes trust now.                                                                                                                                                      |
| `DsaTrust.psm1`                                       | Shared helpers.                                                                                                                                                         |
| `dsa-signing.cer`, `thumbprint.txt`, `SHA256SUMS.txt` | Certificate and hashes. The install checks them first.                                                                                                                  |

`Install-DSA.ps1` checks `thumbprint.txt` against the certificate, and `SHA256SUMS.txt` against the
certificate and setup exe. It stops if either check fails.

The kit cannot vouch for itself: the certificate, `thumbprint.txt` and the hashes all come from the same zip.
So `Install-DSA.ps1` also refuses any certificate that is not self-signed as `CN=Deep Server Analyzer`,
code-signing only, not a CA, RSA 3072 bits or more, and valid for at most 181 days. The real anchor is the
full 40-character thumbprint on the release page:

- Interactive runs print the thumbprint and ask for its last 8 characters.
- `-ExpectedThumbprint <thumbprint>` checks the whole value. It is required with `-NoPrompt` and in
  sessions without a console.
- `DSA-Installer.cmd` shows the full thumbprint and the SHA-256 of `dsa-signing.cer`, and enables **Install**
  only after you paste the thumbprint from the release page.

The data folder `C:\ProgramData\DSA` is created admin-only (SYSTEM and Administrators full control, Users read
only), because the expiry task runs its scripts as SYSTEM. A folder owned by anyone else is renamed to
`DSA.untrusted-<timestamp>`.

## Verifying a download

Each release carries build provenance. Before you copy a file to a server, run:

```powershell
gh attestation verify .\Deep-Server-Analyzer_x.y.z_x64-setup.exe --repo Demonad112/special-memory-analyzer
gh attestation verify .\Deep-Server-Analyzer-usb-kit.zip --repo Demonad112/special-memory-analyzer
```

`SHA256SUMS-release.txt` on the release page lists the hashes of the installer and the kit zip.

## Choosing the trust window

Interactive runs show a picker: 1 h, 4 h, 8 h (default), 12 h, 24 h, custom (`90m`, `2.5h`; clamped to
15 min to 72 h), or remove right after install. Enter, or 30 seconds with no key, picks 8 h.

Non-interactive runs (`-NoPrompt`, or no console) use `-TrustHours` or 8 h, and need `-ExpectedThumbprint`. Pass `-RemoveAfterInstall`
for servers without AppLocker or WDAC rules, where the trust should not outlive the install.

## The countdown

After install the console shows the time left. Keys:

- `E`: extend trust by 1 h
- `R`: remove trust now
- `Q`: close the window. The timer keeps running.

Closing the window never stops the countdown. The scheduled task `DSA-TrustExpiry` removes trust at
expiry. It runs as SYSTEM and also runs at startup, but only removes trust once the expiry has passed.
Scripts and the record live in `C:\ProgramData\DSA`, so the task still works after the USB stick is
unplugged. Every step is logged to `C:\ProgramData\DSA\trust.log`.

## Removing trust early

Run `Remove-Trust.cmd`, or `Show-Trust-Timer.cmd` and press `R`. Either way the certificate is removed
from `LocalMachine\Root` and `LocalMachine\TrustedPublisher`, and the task is unregistered.

## Caveats

- While trust is active, the certificate is trusted machine-wide. Any signed binary from it runs without a prompt.
- AppLocker and WDAC policies can block the setup or the scripts. Check them first.
- Endpoint protection can flag scheduled tasks and certificate changes. Coordinate with the security team.
- Running the kit again restarts the window and replaces the task. It never adds a second copy.

## Test run on a server

1. Run `Install-DSA.cmd -NoPrompt -TrustHours 0.5 -SkipInstall -ExpectedThumbprint <thumbprint from the release page>` and confirm it reports the thumbprint.
2. Check `certlm.msc` for the certificate under Trusted Root and Trusted Publishers.
3. Run `Show-Trust-Timer.cmd`, then `Remove-Trust.cmd`, and confirm the certificate and the
   `DSA-TrustExpiry` task are gone.
