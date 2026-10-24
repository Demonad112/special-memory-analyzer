# Signing options

Decision record for DSA-066. Owner decision Q2 (2026-10-23): keep the hardened self-signed kit for
now, no paid service yet. This page compares it with the main alternative so the choice can be
revisited. Prices and eligibility rules below come from the audit and from vendor pages that change;
check them before buying anything.

## Today: ephemeral self-signed certificate plus the USB kit

`release.yml` creates a self-signed code-signing certificate for each release, signs the installer
and deletes the private key. A release can never be re-signed. Servers trust the certificate
through the USB kit (`installer-kit/`), which adds it to the machine stores for the length of the
install and removes it afterwards (see [usb-install.md](usb-install.md)).

|                |                                                                                                                                                  |
| -------------- | ------------------------------------------------------------------------------------------------------------------------------------------------ |
| Cost           | None                                                                                                                                             |
| SmartScreen    | Still warns. The certificate is not publicly trusted and has no reputation                                                                       |
| Admin burden   | The kit adds a root and a publisher certificate for a time-boxed window, plus a scheduled task for cleanup                                       |
| Secrets        | No signing secret exists: the key is deleted after the build                                                                                     |
| Air-gapped use | Works: everything is on the USB stick                                                                                                            |
| Risk           | The trust step is the sensitive part (DSA-004, DSA-005, DSA-019). The kit was hardened in B02, but it still asks an admin to trust a certificate |

## Option: Azure Trusted Signing

Microsoft's managed signing service. `azure/trusted-signing-action` signs from GitHub Actions, using
short-lived certificates issued from Microsoft's public chain.

|                |                                                                                                                                                        |
| -------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------ |
| Cost           | About USD 10 a month for the basic tier (from the audit; confirm current pricing)                                                                      |
| SmartScreen    | Chains to a publicly trusted root, so no root-store step on clients. Reputation still builds over time, so early warnings are possible                 |
| Admin burden   | None on servers: no certificate to trust, no scheduled task                                                                                            |
| Secrets        | An Azure identity for the workflow. Use OIDC federation, not a stored client secret. The `release` environment already gates the job                   |
| Air-gapped use | Signature verification needs no network when the installer carries a timestamp. Revocation checks still want a connection                              |
| Eligibility    | Requires identity validation of the publisher. Check whether an individual or the owner's organisation qualifies in the owner's region                 |
| Effort         | Replace the certificate step in `release.yml` with the action, add the Azure setup, retire most of `installer-kit/` (a fallback copy is worth keeping) |

## Option: OV certificate in a cloud HSM

A conventional organisation-validated code-signing certificate whose key lives in a vendor or
cloud HSM, since CAs no longer hand out exportable keys. Costs a few hundred USD a year, needs
vendor-specific signing tooling in CI, and SmartScreen reputation is still earned over time. Works
anywhere the CA's chain is trusted, with the same benefit as Trusted Signing but more moving parts.

## What would change

A publicly trusted identity would remove the root-store step, the scheduled task and most of the
USB kit's reason to exist, which closes most of DSA-004, DSA-005 and DSA-019 by design. It does not
remove the need for hash checks (`SHA256SUMS`) or for the `release` approval step.

## Recommendation

Stay on the hardened kit while releases go to a small number of servers the owner controls.
Revisit when any of these is true: more admins than the owner install it, a client refuses the
root-trust step, or SmartScreen warnings cost real support time. At that point try Azure Trusted
Signing first, and keep the USB kit for air-gapped installs.
