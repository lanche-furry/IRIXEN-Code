# Sign Windows binaries with a code-signing certificate supplied as a base64 PFX.
#
#   $env:SIGN_PFX          = '<base64 of the .pfx>'
#   $env:SIGN_PFX_PASSWORD = '<password>'
#   ./scripts/sign-windows.ps1 -Files irixen-code.exe,'IRIXEN Code_*-setup.exe'
#
# A self-signed certificate is accepted. It puts a real signature and a
# publisher name on the file, which is what "unsigned binary" heuristics look
# for, but Windows will still show "Unknown publisher" until the root is
# trusted on that machine — it is a stopgap, not a replacement for an OV/EV
# certificate.

[CmdletBinding()]
param(
    [Parameter(Mandatory = $true, ValueFromRemainingArguments = $true)]
    [string[]] $Files
)

$ErrorActionPreference = 'Stop'

if (-not $env:SIGN_PFX) { throw 'SIGN_PFX is empty — nothing to sign with.' }
if (-not $env:SIGN_PFX_PASSWORD) { throw 'SIGN_PFX_PASSWORD is empty.' }

$resolved = @()
foreach ($pattern in $Files) {
    $hits = @(Get-ChildItem -Path $pattern -ErrorAction SilentlyContinue)
    if ($hits.Count -eq 0) { throw "nothing matches $pattern" }
    $resolved += $hits | ForEach-Object { $_.FullName }
}

$pfxPath = Join-Path ([IO.Path]::GetTempPath()) 'irixen-signing.pfx'
try {
    [IO.File]::WriteAllBytes($pfxPath, [Convert]::FromBase64String($env:SIGN_PFX))
    $secure = ConvertTo-SecureString $env:SIGN_PFX_PASSWORD -AsPlainText -Force
    $cert = Import-PfxCertificate -FilePath $pfxPath -CertStoreLocation 'Cert:\CurrentUser\My' -Password $secure
}
finally {
    Remove-Item $pfxPath -Force -ErrorAction SilentlyContinue
}

Write-Host "signing with $($cert.Subject)"

foreach ($file in $resolved) {
    $sig = Set-AuthenticodeSignature -FilePath $file -Certificate $cert `
        -TimestampServer 'http://timestamp.digicert.com' -HashAlgorithm SHA256
    Write-Host ("  {0}: {1}" -f (Split-Path $file -Leaf), $sig.Status)
    # A self-signed chain reports UnknownError: the file *is* signed, the root
    # is simply not trusted here. Anything else is a real failure.
    if ($sig.Status -notin @('Valid', 'UnknownError')) {
        throw "signing failed for $file : $($sig.StatusMessage)"
    }
}

foreach ($file in $resolved) {
    $check = Get-AuthenticodeSignature -FilePath $file
    $signer = if ($check.SignerCertificate) { $check.SignerCertificate.Subject } else { '<none>' }
    Write-Host ("  verify {0}: {1} — {2}" -f (Split-Path $file -Leaf), $check.Status, $signer)
    if (-not $check.SignerCertificate) { throw "$file has no signature after signing" }
}
