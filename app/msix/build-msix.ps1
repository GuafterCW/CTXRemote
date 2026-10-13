# Packs the built app and service into MSIX packages (see docs/DEPLOY.md):
#   CTXRemote-<version>-store.msix  unsigned, for upload to Partner Center (the store signs it)
#   CTXRemote-<version>.msix        signed, for installing directly as a test
#   CTXRemote-Test.cer              the test signature's certificate
# Runs after `npm run tauri build` on Windows, from the repository root; $BinDir
# holds ctxremote.exe and ctxremote-service.exe from that build.
param(
    [Parameter(Mandatory)] [string] $Version,
    [string] $BinDir = "target\release",
    [string] $IdentityName = "CTXRemote",
    [string] $Publisher = "CN=CTXRemote Test",
    [string] $PublisherDisplayName = "CTXRemote",
    [string] $OutDir = "out",
    # Base64 of a .pfx whose subject equals $Publisher; without it a new test certificate is made.
    [string] $CertBase64 = "",
    [string] $CertPassword = ""
)
$ErrorActionPreference = "Stop"

# MSIX wants four parts; the store wants the last one to be 0.
$msixVersion = "$Version.0"
$layout = Join-Path $env:RUNNER_TEMP "msix-layout"
Remove-Item -Recurse -Force $layout -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force "$layout\Assets", $OutDir | Out-Null

Copy-Item "$BinDir\ctxremote.exe", "$BinDir\ctxremote-service.exe" $layout
# Logos in every size Windows asks for. The "unplated" ones keep the taskbar
# from putting the accent colour behind the icon.
Copy-Item app\msix\Assets\*.png "$layout\Assets\"
$manifest = Get-Content -Raw -Encoding UTF8 app\msix\AppxManifest.xml
$manifest = $manifest.Replace('$IDENTITY_NAME$', $IdentityName).Replace('$PUBLISHER$', [Security.SecurityElement]::Escape($Publisher))
$manifest = $manifest.Replace('$PUBLISHER_DISPLAY_NAME$', [Security.SecurityElement]::Escape($PublisherDisplayName)).Replace('$VERSION$', $msixVersion)
[IO.File]::WriteAllText("$layout\AppxManifest.xml", $manifest, (New-Object Text.UTF8Encoding $false))

$sdk = Get-ChildItem "${env:ProgramFiles(x86)}\Windows Kits\10\bin\*\x64\makeappx.exe" | Sort-Object FullName | Select-Object -Last 1
if (-not $sdk) { throw "makeappx.exe nicht gefunden (Windows SDK)" }
$bin = $sdk.DirectoryName
Write-Host "Windows SDK: $bin"

# resources.pri tells Windows which logo file is which size and variant.
$priConfig = Join-Path $env:RUNNER_TEMP "priconfig.xml"
& "$bin\makepri.exe" createconfig /cf $priConfig /dq de-DE /pv 10.0.0 /o
if ($LASTEXITCODE) { throw "makepri createconfig fehlgeschlagen" }
& "$bin\makepri.exe" new /pr $layout /cf $priConfig /mn "$layout\AppxManifest.xml" /of "$layout\resources.pri" /o
if ($LASTEXITCODE) { throw "makepri new fehlgeschlagen" }

$store = Join-Path $OutDir "CTXRemote-$Version-store.msix"
& "$bin\makeappx.exe" pack /o /d $layout /p $store
if ($LASTEXITCODE) { throw "makeappx fehlgeschlagen" }

# The test package: same content, signed so Windows installs it directly.
$signed = Join-Path $OutDir "CTXRemote-$Version.msix"
Copy-Item $store $signed
$pfx = Join-Path $env:RUNNER_TEMP "msix.pfx"
if ($CertBase64) {
    [IO.File]::WriteAllBytes($pfx, [Convert]::FromBase64String($CertBase64))
    $password = $CertPassword
} else {
    Write-Host "::warning::Kein MSIX_CERT-Secret: Testpaket mit neuem Zertifikat signiert, das bei jedem Release neu vertraut werden muss"
    $password = [Guid]::NewGuid().ToString()
    $cert = New-SelfSignedCertificate -Type Custom -Subject $Publisher -KeyUsage DigitalSignature `
        -FriendlyName "CTXRemote Test" -CertStoreLocation Cert:\CurrentUser\My `
        -TextExtension @("2.5.29.37={text}1.3.6.1.5.5.7.3.3", "2.5.29.19={text}")
    Export-PfxCertificate -Cert $cert -FilePath $pfx -Password (ConvertTo-SecureString $password -AsPlainText -Force) | Out-Null
}
& "$bin\signtool.exe" sign /fd SHA256 /f $pfx /p $password $signed
if ($LASTEXITCODE) { throw "signtool fehlgeschlagen" }
$loaded = New-Object Security.Cryptography.X509Certificates.X509Certificate2($pfx, $password)
[IO.File]::WriteAllBytes((Join-Path $OutDir "CTXRemote-Test.cer"), $loaded.Export("Cert"))
Remove-Item $pfx
Get-ChildItem $OutDir
