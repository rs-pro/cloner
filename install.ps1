#Requires -Version 5.1
<#
Build and install the Rust `cloner` binary on Windows.

Usage:
  .\install.ps1                    # install into %USERPROFILE%\.local\bin
  .\install.ps1 -Prefix C:\tools   # install into C:\tools\bin
  .\install.ps1 -System            # install into %ProgramFiles%\cloner\bin (elevated)
  .\install.ps1 -NoDeps            # skip Visual Studio Build Tools / Rust install
  .\install.ps1 -NoBuild           # install an already built binary
  .\install.ps1 -DryRun            # print what would happen
#>
[CmdletBinding()]
param(
    [string]$Prefix,
    [switch]$System,
    [switch]$NoDeps,
    [switch]$NoBuild,
    [switch]$NoInstall,
    [switch]$AddToPath,
    [string]$Toolchain,
    [switch]$DryRun,
    [switch]$Help
)

$ErrorActionPreference = 'Stop'

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
if (-not $Prefix) { $Prefix = Join-Path $env:USERPROFILE '.local' }
if (-not $Toolchain) {
    if ($env:CLONER_RUST_TOOLCHAIN) { $Toolchain = $env:CLONER_RUST_TOOLCHAIN } else { $Toolchain = 'stable' }
}
if ($System) {
    $Prefix = Join-Path $env:ProgramFiles 'cloner'
}
$BinDir = Join-Path $Prefix 'bin'

function Write-Log($message) { Write-Host "==> $message" -ForegroundColor Cyan }
function Write-Note($message) { Write-Host "    $message" }
function Write-Warn2($message) { Write-Host "warning: $message" -ForegroundColor Yellow }
function Stop-WithError($message) {
    Write-Host "error: $message" -ForegroundColor Red
    exit 1
}

function Show-Usage {
@'
Build and install the Rust cloner binary (Windows).

Usage: .\install.ps1 [OPTIONS]

Options:
  -Prefix DIR      Install the binary into DIR\bin (default: %USERPROFILE%\.local)
  -System          Install into %ProgramFiles%\cloner\bin (run elevated)
  -NoDeps          Do not install Visual Studio C++ Build Tools or Rust
  -NoBuild         Do not run cargo build; install target\release\cloner.exe
  -NoInstall       Build only; do not copy the binary anywhere
  -AddToPath       Persist the bin directory in the user PATH
  -Toolchain NAME  Rust toolchain for rustup (default: stable)
  -DryRun          Print commands instead of running them
  -Help            Show this help

Environment:
  CLONER_RUST_TOOLCHAIN   Rust toolchain for rustup (default: stable)

Examples:
  .\install.ps1
  .\install.ps1 -AddToPath
  .\install.ps1 -Prefix C:\tools -NoDeps
'@
}

function Invoke-Step {
    param([string]$Label, [scriptblock]$Action)
    if ($DryRun) {
        Write-Host "  [dry-run] $Label"
    }
    else {
        Write-Host "  > $Label"
        # Native tools (cargo, winget) write progress to stderr. Under
        # ErrorActionPreference='Stop' PowerShell turns that into a terminating
        # NativeCommandError, so relax it just around the command.
        $previous = $ErrorActionPreference
        $ErrorActionPreference = 'Continue'
        try {
            & $Action
        }
        finally {
            $ErrorActionPreference = $previous
        }
        if ($LASTEXITCODE -ne 0) { Stop-WithError "$Label failed (exit $LASTEXITCODE)" }
    }
}

function Test-Command($name) {
    return [bool](Get-Command $name -ErrorAction SilentlyContinue)
}

function Test-IsAdmin {
    $id = [Security.Principal.WindowsIdentity]::GetCurrent()
    return ([Security.Principal.WindowsPrincipal]::new($id)).IsInRole(
        [Security.Principal.WindowsBuiltInRole]::Administrator)
}

function Get-HostTriple {
    if ($DryRun -or -not (Test-Command rustc)) { return 'x86_64-pc-windows-msvc' }
    $line = (& rustc -vV 2>$null | Select-String -Pattern '^host: ')
    if ($line) { return ($line -split '\s+')[1] }
    return 'x86_64-pc-windows-msvc'
}

# Locates Visual Studio C++ Build Tools via vswhere. Returns the install root.
function Find-VisualStudio {
    $vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
    if (-not (Test-Path $vswhere)) { return $null }
    $path = & $vswhere -latest -products * `
        -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 `
        -property installationPath 2>$null | Select-Object -First 1
    if ($path -and (Test-Path $path)) { return $path }
    return $null
}

# Imports the vcvars64.bat environment (cl.exe, INCLUDE, LIB) into this process.
function Import-VcVars([string]$vsRoot) {
    $vcvars = Join-Path $vsRoot 'VC\Auxiliary\Build\vcvars64.bat'
    if (-not (Test-Path $vcvars)) { return $false }
    try {
        $dump = & cmd.exe /c "call `"$vcvars`" >nul 2>&1 && set"
    }
    catch {
        return $false
    }
    if ($LASTEXITCODE -ne 0 -or -not $dump) { return $false }
    foreach ($entry in $dump) {
        $idx = $entry.IndexOf('=')
        if ($idx -lt 1) { continue }
        $name = $entry.Substring(0, $idx)
        $value = $entry.Substring($idx + 1)
        Set-Item -Path "Env:$name" -Value $value
    }
    return $true
}

function Install-CToolchain {
    if ($NoDeps) {
        Write-Log 'skipping dependency installation (-NoDeps)'
        return
    }

    $hostTriple = Get-HostTriple
    if ($hostTriple -like '*-gnu') {
        if (Test-Command gcc) {
            Write-Log "using existing gcc: $((Get-Command gcc).Source)"
        }
        else {
            Stop-WithError "the $hostTriple Rust toolchain needs gcc on PATH; install MSYS2 or switch to rustup default stable (MSVC)"
        }
        return
    }

    if (Test-Command cl) {
        Write-Log "using existing MSVC compiler: $((Get-Command cl).Source)"
        return
    }

    $vs = Find-VisualStudio
    if ($vs) {
        Write-Log "found Visual Studio C++ Build Tools: $vs"
        if (-not $DryRun) {
            if (-not (Import-VcVars $vs)) {
                Write-Warn2 'could not import the vcvars64 environment; build may fail to link'
            }
        }
        return
    }

    Write-Log 'installing Visual Studio 2022 Build Tools (C++ toolchain)'
    if (-not (Test-Command winget)) {
        Stop-WithError 'no C++ compiler found and winget is unavailable. Install "Desktop development with C++" from https://visualstudio.microsoft.com/visual-cpp-build-tools/ and re-run, or use -NoDeps with an existing toolchain'
    }
    Invoke-Step 'winget install Microsoft.VisualStudio.2022.BuildTools' {
        & winget install --id Microsoft.VisualStudio.2022.BuildTools --exact --accept-package-agreements --accept-source-agreements --silent --override '--quiet --wait --norestart --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended'
    }
    if ($DryRun) { return }

    $vs = Find-VisualStudio
    if (-not $vs) {
        Stop-WithError 'Build Tools installed, but the C++ toolset was not found. Open a new shell and re-run, or install the "Desktop development with C++" workload manually'
    }
    if (-not (Import-VcVars $vs)) {
        Stop-WithError 'the C++ toolchain is installed but its environment could not be loaded; open a "x64 Native Tools Command Prompt" and re-run'
    }
}

function Install-Rust {
    if (Test-Command cargo) {
        Write-Log "using existing cargo: $((Get-Command cargo).Source)"
        return
    }

    Write-Log "installing Rust via rustup ($Toolchain)"
    if ($DryRun) {
        Write-Host '  [dry-run] download rustup-init.exe from https://win.rustup.rs/x86_64 and run it with -y --profile minimal --default-toolchain'
        return
    }

    $init = Join-Path $env:TEMP 'rustup-init.exe'
    try {
        [Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
        Invoke-WebRequest -UseBasicParsing -Uri 'https://win.rustup.rs/x86_64' -OutFile $init
    }
    catch {
        Stop-WithError "could not download rustup-init.exe: $($_.Exception.Message). Install Rust from https://rustup.rs and re-run with -NoDeps"
    }
    & $init -y --profile minimal --default-toolchain $Toolchain
    if ($LASTEXITCODE -ne 0) { Stop-WithError "rustup installation failed (exit $LASTEXITCODE)" }
    Remove-Item $init -Force -ErrorAction SilentlyContinue

    $cargoBin = Join-Path $env:USERPROFILE '.cargo\bin'
    if (Test-Path $cargoBin) {
        $env:PATH = "$cargoBin;$env:PATH"
    }
    if (-not (Test-Command cargo)) {
        Stop-WithError 'cargo was not found after installing Rust; open a new shell and retry'
    }
}

function Build-Cloner {
    if ($NoBuild) {
        Write-Log 'skipping build (-NoBuild)'
        return
    }
    if (-not (Test-Path (Join-Path $ScriptDir 'Cargo.toml'))) {
        Stop-WithError "Cargo.toml not found in $ScriptDir"
    }
    Write-Log 'building cloner (release)'
    Push-Location $ScriptDir
    try {
        Invoke-Step 'cargo build --release --locked' { & cargo build --release --locked }
    }
    finally {
        Pop-Location
    }
}

function Install-Binary {
    if ($NoInstall) {
        Write-Log 'skipping binary installation (-NoInstall)'
        return
    }

    $source = Join-Path $ScriptDir 'target\release\cloner.exe'
    $destination = Join-Path $BinDir 'cloner.exe'

    if (-not $DryRun -and -not (Test-Path $source)) {
        Stop-WithError "built binary not found at $source; run without -NoBuild"
    }

    Write-Log "installing to $destination"
    if ($System -and -not (Test-IsAdmin)) {
        Stop-WithError '-System needs administrator rights. Re-run PowerShell as Administrator, or install into the user prefix (default)'
    }

    Invoke-Step "new-item $BinDir" { New-Item -ItemType Directory -Force -Path $BinDir | Out-Null }
    Invoke-Step "copy $source -> $destination" { Copy-Item -Force $source $destination }

    $onPath = ($env:PATH -split ';' | Where-Object { $_ -and $_.TrimEnd('\') -eq $BinDir.TrimEnd('\') }).Count -gt 0
    if (-not $onPath) {
        Write-Warn2 "$BinDir is not in PATH"
        if ($AddToPath -and -not $DryRun) {
            $userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
            if ($userPath -notlike "*$BinDir*") {
                [Environment]::SetEnvironmentVariable('Path', "$BinDir;$userPath", 'User')
                Write-Log "added $BinDir to the user PATH"
            }
            $env:PATH = "$BinDir;$env:PATH"
            Write-Note 'open a new shell so other terminals pick it up'
        }
        else {
            Write-Host "`n  Add this to your PowerShell profile:`n"
            Write-Host "    `$env:PATH = `"$BinDir;`$env:PATH`"`n"
            Write-Host '  or re-run with -AddToPath to persist it for your user.'
        }
    }

    if (-not $DryRun) {
        & $destination --version
        if ($LASTEXITCODE -ne 0) { Stop-WithError "$destination --version failed" }
    }
}

function Main {
    if ($Help) { Show-Usage; return }

    Write-Log 'cloner installer (Windows)'
    # Rust first: Install-CToolchain asks rustc which target triple (MSVC vs
    # GNU) the toolchain manager will actually use.
    Install-Rust
    Install-CToolchain
    Build-Cloner
    Install-Binary

    if ($DryRun) {
        Write-Log 'dry run complete'
        return
    }

    Write-Log 'done'
    if ($NoInstall) { return }
    if (Test-Command cloner) {
        Write-Host "`nRun:`n`n  cloner --help`n  cloner validate`n  cloner run`n"
    }
    else {
        Write-Host "`nRun:`n`n  `"$BinDir\cloner.exe`" --help`n"
    }
}

Main
