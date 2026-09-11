[CmdletBinding()]
param(
    [ValidateNotNullOrEmpty()]
    [string]$IdentityName = "Enrell.RelayHop",

    [ValidateNotNullOrEmpty()]
    [string]$Publisher = "CN=2E344261-24C2-4E99-88F4-1C56A5C4DDE4",

    [ValidateNotNullOrEmpty()]
    [string]$PublisherDisplayName = "Enrell",

    [string]$DisplayName = "RelayHop",
    [string]$Version
)

$ErrorActionPreference = "Stop"
$ProjectRoot = Split-Path -Parent $PSScriptRoot
$ManifestTemplate = Join-Path $PSScriptRoot "msix\AppxManifest.template.xml"
$CargoManifest = Join-Path $ProjectRoot "Cargo.toml"
$StageRoot = Join-Path $ProjectRoot "target\msix\x64"
$AssetsRoot = Join-Path $StageRoot "Assets"
$DistRoot = Join-Path $ProjectRoot "dist"

if (-not $Version) {
    $cargoText = Get-Content $CargoManifest -Raw
    $match = [regex]::Match($cargoText, '(?m)^version\s*=\s*"(\d+)\.(\d+)\.(\d+)"')
    if (-not $match.Success) {
        throw "Não foi possível ler a versão do Cargo.toml."
    }
    $Version = "{0}.{1}.{2}.0" -f $match.Groups[1].Value, $match.Groups[2].Value, $match.Groups[3].Value
}
if ($Version -notmatch '^\d+\.\d+\.\d+\.\d+$') {
    throw "A versão do MSIX deve conter quatro números, por exemplo 0.2.2.0."
}

$cargo = Join-Path $env:USERPROFILE ".cargo\bin\cargo.exe"
if (-not (Test-Path -LiteralPath $cargo)) {
    $cargo = (Get-Command cargo.exe -ErrorAction Stop).Source
}

$makeAppx = Get-Command makeappx.exe -ErrorAction SilentlyContinue
if ($makeAppx) {
    $makeAppxPath = $makeAppx.Source
} else {
    $kitsBin = "${env:ProgramFiles(x86)}\Windows Kits\10\bin"
    $makeAppxPath = Get-ChildItem -LiteralPath $kitsBin -Filter makeappx.exe -Recurse |
        Where-Object { $_.FullName -match '\\x64\\makeappx\.exe$' } |
        Sort-Object FullName -Descending |
        Select-Object -First 1 -ExpandProperty FullName
}
if (-not $makeAppxPath) {
    throw "MakeAppx.exe não foi encontrado. Instale o Windows SDK."
}

& $cargo build --release --locked
if ($LASTEXITCODE -ne 0) {
    throw "A compilação release falhou."
}

if (Test-Path -LiteralPath $StageRoot) {
    Remove-Item -LiteralPath $StageRoot -Recurse -Force
}
New-Item -ItemType Directory -Path $AssetsRoot -Force | Out-Null
New-Item -ItemType Directory -Path $DistRoot -Force | Out-Null
Copy-Item -LiteralPath (Join-Path $ProjectRoot "target\release\relayhop.exe") -Destination $StageRoot

Add-Type -AssemblyName System.Drawing
$sourceIcon = [Drawing.Image]::FromFile((Join-Path $ProjectRoot "assets\icon-512.png"))
try {
    $icons = @{
        "StoreLogo.png" = 50
        "Square44x44Logo.png" = 44
        "Square150x150Logo.png" = 150
    }
    foreach ($entry in $icons.GetEnumerator()) {
        $bitmap = [Drawing.Bitmap]::new($entry.Value, $entry.Value)
        try {
            $graphics = [Drawing.Graphics]::FromImage($bitmap)
            try {
                $graphics.InterpolationMode = [Drawing.Drawing2D.InterpolationMode]::HighQualityBicubic
                $graphics.DrawImage($sourceIcon, 0, 0, $entry.Value, $entry.Value)
            } finally {
                $graphics.Dispose()
            }
            $bitmap.Save((Join-Path $AssetsRoot $entry.Key), [Drawing.Imaging.ImageFormat]::Png)
        } finally {
            $bitmap.Dispose()
        }
    }
} finally {
    $sourceIcon.Dispose()
}

function ConvertTo-XmlAttribute([string]$Value) {
    return [Security.SecurityElement]::Escape($Value)
}

$manifest = Get-Content $ManifestTemplate -Raw
$manifest = $manifest.Replace("__IDENTITY_NAME__", (ConvertTo-XmlAttribute $IdentityName))
$manifest = $manifest.Replace("__PUBLISHER__", (ConvertTo-XmlAttribute $Publisher))
$manifest = $manifest.Replace("__PUBLISHER_DISPLAY_NAME__", (ConvertTo-XmlAttribute $PublisherDisplayName))
$manifest = $manifest.Replace("__DISPLAY_NAME__", (ConvertTo-XmlAttribute $DisplayName))
$manifest = $manifest.Replace("__VERSION__", $Version)
$manifestPath = Join-Path $StageRoot "AppxManifest.xml"
[IO.File]::WriteAllText($manifestPath, $manifest, [Text.UTF8Encoding]::new($false))

$packagePath = Join-Path $DistRoot "RelayHop_${Version}_x64.msix"
& $makeAppxPath pack /o /h SHA256 /d $StageRoot /p $packagePath
if ($LASTEXITCODE -ne 0) {
    throw "MakeAppx falhou ao criar o pacote."
}

Write-Host "MSIX criado em $packagePath"
Write-Host "Este pacote é para envio ao Partner Center; a Microsoft o assina após a certificação."
