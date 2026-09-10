# RelayHop installer for Windows (x64).
#
#   irm https://raw.githubusercontent.com/enrell/relayhop/main/packaging/install.ps1 | iex
#
# Pinned version:
#   $env:RELAYHOP_VERSION = "v0.2.0"; irm ... | iex
#
# Installs to %LOCALAPPDATA%\RelayHop (no admin), verifies SHA-256,
# and creates a Start Menu shortcut. The binary is unsigned, so
# SmartScreen may ask for confirmation on first run.
$ErrorActionPreference = "Stop"
$ProgressPreference = "SilentlyContinue"

$Repo = "enrell/relayhop"
$Version = $env:RELAYHOP_VERSION
if (-not $Version) { $Version = "latest" }
$BaseUrl = $env:RELAYHOP_BASE_URL
if (-not $BaseUrl) {
    if ($Version -eq "latest") {
        $BaseUrl = "https://github.com/$Repo/releases/latest/download"
    } else {
        if (-not $Version.StartsWith("v")) { $Version = "v$Version" }
        $BaseUrl = "https://github.com/$Repo/releases/download/$Version"
    }
}

if (-not [Environment]::Is64BitOperatingSystem) {
    throw "RelayHop: apenas Windows x64 por enquanto."
}
if ([Net.ServicePointManager]::SecurityProtocol -band [Net.SecurityProtocolType]::Tls12 -eq 0) {
    [Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12
}

$InstallDir = Join-Path $env:LOCALAPPDATA "RelayHop"
$Tmp = Join-Path ([IO.Path]::GetTempPath()) ("relayhop-" + [Guid]::NewGuid().ToString("N"))
New-Item -ItemType Directory $Tmp -Force | Out-Null
try {
    Write-Host "Baixando RelayHop $Version..."
    Invoke-WebRequest "$BaseUrl/relayhop.exe" -OutFile (Join-Path $Tmp "relayhop.exe")
    Invoke-WebRequest "$BaseUrl/relayhop.exe.sha256" -OutFile (Join-Path $Tmp "relayhop.exe.sha256")

    Write-Host "Verificando integridade..."
    $expected = ((Get-Content (Join-Path $Tmp "relayhop.exe.sha256") -Raw) -split '\s+')[0].ToLower()
    $actual = (Get-FileHash (Join-Path $Tmp "relayhop.exe") -Algorithm SHA256).Hash.ToLower()
    if ($expected -ne $actual) { throw "SHA-256 não confere. Download corrompido ou adulterado." }

    New-Item -ItemType Directory $InstallDir -Force | Out-Null
    Move-Item (Join-Path $Tmp "relayhop.exe") (Join-Path $InstallDir "relayhop.exe") -Force

    $shortcut = Join-Path ([Environment]::GetFolderPath("Programs")) "RelayHop.lnk"
    $shell = New-Object -ComObject WScript.Shell
    $link = $shell.CreateShortcut($shortcut)
    $link.TargetPath = Join-Path $InstallDir "relayhop.exe"
    $link.WorkingDirectory = $InstallDir
    $link.Description = "RelayHop — salto temporário pelo Tor para abrir o Discord"
    $link.Save()

    Write-Host ""
    Write-Host "RelayHop instalado em $InstallDir\relayhop.exe"
    Write-Host "Atalho criado no Menu Iniciar. Na primeira execução o SmartScreen pode pedir confirmação (binário sem assinatura)."
} finally {
    Remove-Item $Tmp -Recurse -Force -ErrorAction SilentlyContinue
}
