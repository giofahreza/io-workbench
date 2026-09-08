# Install io-workbench from a GitHub Release into the current user's profile.
#
# In an interactive terminal the installer asks for the small set of choices
# that change the host's authority or lifecycle. It never asks for passwords,
# tokens, OTP secrets, API keys, or provider credentials.

[CmdletBinding()]
param(
    [string]$Version = $env:IO_WORKBENCH_VERSION,
    [string]$Repository = $env:IO_WORKBENCH_REPOSITORY,
    [string]$InstallDir = $env:IO_WORKBENCH_INSTALL_DIR,
    [string]$BindHost = $env:IO_WORKBENCH_HOST,
    [string]$Port = $env:IO_WORKBENCH_PORT,
    [string]$ConfigDir = $env:IO_WORKBENCH_CONFIG_DIR,
    [string]$WorkspaceRoot = $env:IO_WORKBENCH_WORKSPACE_ROOT,
    [string]$NpmPrefix = $env:IO_WORKBENCH_NPM_PREFIX,
    [Alias('WithIoGateway')]
    [switch]$InstallIoGateway,
    [Alias('WithoutIoGateway')]
    [switch]$NoIoGateway,
    [Alias('WithCodex')]
    [switch]$InstallCodex,
    [Alias('WithoutCodex')]
    [switch]$NoCodex,
    [Alias('WithClaude')]
    [switch]$InstallClaude,
    [Alias('WithoutClaude')]
    [switch]$NoClaude,
    [Alias('WithGemini')]
    [switch]$InstallGemini,
    [Alias('WithoutGemini')]
    [switch]$NoGemini,
    [switch]$ConfigureClis,
    [switch]$NoConfigureClis,
    [switch]$CheckProviders,
    [switch]$NoCheckProviders,
    [switch]$TestProviders,
    [switch]$NoTestProviders,
    [switch]$AutoStart,
    [switch]$NoAutoStart,
    [switch]$NoStart,
    [switch]$Interactive,
    [switch]$NonInteractive,
    [Parameter(ValueFromRemainingArguments = $true)]
    [string[]]$RemainingArguments
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$WorkbenchTaskName = 'io-workbench'
$ManagedTaskMarker = 'Managed by io-workbench install.ps1'
$ManagedLauncherMarker = '# Managed by io-workbench install.ps1'

function Write-Note {
    param([string]$Message)
    Write-Host "io-workbench installer: $Message"
}

function Write-WarningNote {
    param([string]$Message)
    Write-Warning "io-workbench installer: $Message"
}

function Show-Usage {
    @'
Usage: install.ps1 [options]

Installs the matching io-workbench GitHub Release for this Windows computer.
An interactive terminal receives a safe first-run setup questionnaire.

Release options:
  -Version <tag>             Install v0.1.0 (or 0.1.0), not latest.
  --version <tag>            POSIX-style spelling of -Version.

Runtime options:
  -BindHost <IP>             Bind address (default: 127.0.0.1).
  --host <IP>                POSIX-style spelling of -BindHost.
  -Port <port>               TCP port (default: 8787).
  --port <port>              POSIX-style spelling of -Port.
  -ConfigDir <path>          Data directory (default: %USERPROFILE%\.io-workbench).
  --config-dir <path>        POSIX-style spelling of -ConfigDir.
  -WorkspaceRoot <path>      Existing directory the UI may browse.
  --workspace-root <path>    POSIX-style spelling of -WorkspaceRoot.
  -AutoStart                 Register and start a per-user Scheduled Task.
  -NoAutoStart               Do not create one; removes an installer-managed
                             task when one exists.
  -NoStart                   Alias for -NoAutoStart.
  --autostart | --no-autostart | --no-start

Optional host tools:
  -InstallIoGateway | -NoIoGateway
  -InstallCodex | -NoCodex
  -InstallClaude | -NoClaude
  -InstallGemini | -NoGemini
  -ConfigureClis | -NoConfigureClis
  -CheckProviders | -NoCheckProviders
                         Check local version/auth status (no model request).
  -TestProviders | -NoTestProviders
                         Send one minimal read-only request per available CLI;
                         may use provider quota or incur cost.
  --with-io-gateway | --without-io-gateway
  --with-codex | --without-codex
  --with-claude | --without-claude
  --with-gemini | --without-gemini
  --configure-clis | --no-configure-clis
  --check-providers | --no-check-providers
  --test-providers | --no-test-providers

Interaction:
  -Interactive               Require questions in a controlling terminal.
  -NonInteractive            Use explicit options and safe defaults.
  --interactive | --non-interactive
  -Help                       Show this help.

Environment overrides:
  IO_WORKBENCH_VERSION, IO_WORKBENCH_REPOSITORY, IO_WORKBENCH_INSTALL_DIR
  IO_WORKBENCH_HOST, IO_WORKBENCH_PORT, IO_WORKBENCH_CONFIG_DIR
  IO_WORKBENCH_WORKSPACE_ROOT, IO_WORKBENCH_NPM_PREFIX
  IO_WORKBENCH_AUTOSTART, IO_WORKBENCH_INSTALL_IO_GATEWAY
  IO_WORKBENCH_INSTALL_CODEX, IO_WORKBENCH_INSTALL_CLAUDE
  IO_WORKBENCH_INSTALL_GEMINI, IO_WORKBENCH_CONFIGURE_CLIS
  IO_WORKBENCH_CHECK_PROVIDERS, IO_WORKBENCH_TEST_PROVIDERS
  IO_WORKBENCH_INTERACTIVE

Choice values are auto, yes, or no. Non-interactive auto makes no
Scheduled-Task, IO Gateway, provider-CLI, provider-login, provider-check, or
live-model-request changes.
'@ | Write-Host
}

function Get-RemainingOptionValue {
    param(
        [string[]]$Arguments,
        [ref]$Index,
        [string]$OptionName,
        [string]$ValueDescription = 'a value'
    )

    if (($Index.Value + 1) -ge $Arguments.Count) {
        throw "$OptionName needs $ValueDescription."
    }
    $Index.Value++
    return $Arguments[$Index.Value]
}

function ConvertTo-InstallerMode {
    param(
        [string]$Value,
        [string]$Name
    )

    if ([string]::IsNullOrWhiteSpace($Value)) {
        throw "$Name must be auto, yes, or no."
    }
    switch ($Value.Trim().ToLowerInvariant()) {
        'auto' { return 'auto' }
        '1' { return 'yes' }
        'true' { return 'yes' }
        'yes' { return 'yes' }
        'y' { return 'yes' }
        'on' { return 'yes' }
        '0' { return 'no' }
        'false' { return 'no' }
        'no' { return 'no' }
        'n' { return 'no' }
        'off' { return 'no' }
        default { throw "$Name must be auto, yes, or no." }
    }
}

function ConvertTo-InstallerPort {
    param(
        [string]$Value,
        [string]$Name = 'Port'
    )

    [int]$parsedPort = 0
    if ([string]::IsNullOrWhiteSpace($Value) -or
        -not [int]::TryParse($Value, [ref]$parsedPort) -or
        $parsedPort -lt 1 -or $parsedPort -gt 65535) {
        throw "$Name must be a TCP port number from 1 through 65535."
    }
    return $parsedPort
}

function Get-UserProfilePath {
    if (-not [string]::IsNullOrWhiteSpace($env:USERPROFILE)) {
        return [System.IO.Path]::GetFullPath($env:USERPROFILE)
    }
    $profile = [Environment]::GetFolderPath('UserProfile')
    if ([string]::IsNullOrWhiteSpace($profile)) {
        throw 'Could not determine the current user profile directory.'
    }
    return [System.IO.Path]::GetFullPath($profile)
}

function Resolve-InstallerPath {
    param(
        [string]$Value,
        [string]$Name,
        [string]$UserProfile
    )

    if ([string]::IsNullOrWhiteSpace($Value)) {
        throw "$Name cannot be empty."
    }
    $expanded = [Environment]::ExpandEnvironmentVariables($Value.Trim())
    if ($expanded -eq '~') {
        $expanded = $UserProfile
    }
    elseif ($expanded -match '^~[\\/]') {
        $expanded = Join-Path $UserProfile $expanded.Substring(2)
    }
    try {
        return [System.IO.Path]::GetFullPath($expanded)
    }
    catch {
        throw "$Name is not a valid path: $Value"
    }
}

function ConvertTo-PowerShellLiteral {
    param([string]$Value)
    return "'$($Value.Replace("'", "''"))'"
}

function ConvertTo-InstallerIPAddress {
    param(
        [string]$Value,
        [string]$Name = 'Bind host'
    )

    [System.Net.IPAddress]$address = $null
    if ([string]::IsNullOrWhiteSpace($Value) -or
        -not [System.Net.IPAddress]::TryParse($Value.Trim(), [ref]$address)) {
        throw "$Name must be an IPv4 or IPv6 address literal, such as 127.0.0.1 or 0.0.0.0."
    }
    return $address
}

function Test-InstallerLoopbackAddress {
    param([System.Net.IPAddress]$Address)
    return [System.Net.IPAddress]::IsLoopback($Address)
}

function Test-InstallerPortInUse {
    param([int]$Port)
    try {
        $listeners = [System.Net.NetworkInformation.IPGlobalProperties]::GetIPGlobalProperties().GetActiveTcpListeners()
        foreach ($listener in $listeners) {
            if ($listener.Port -eq $Port) {
                return $true
            }
        }
    }
    catch {
        # Lack of listener-inspection support should not prevent an install.
    }
    return $false
}

function Test-InstallerInteractive {
    param([bool]$Disabled)

    if ($Disabled -or -not [string]::IsNullOrWhiteSpace($env:CI)) {
        return $false
    }
    try {
        if (-not [Environment]::UserInteractive -or [Console]::IsInputRedirected) {
            return $false
        }
        return $null -ne $Host -and $null -ne $Host.UI -and $null -ne $Host.UI.RawUI
    }
    catch {
        return $false
    }
}

function Read-InstallerYesNo {
    param(
        [string]$Prompt,
        [bool]$Default
    )

    $defaultHint = if ($Default) { 'Y/n' } else { 'y/N' }
    while ($true) {
        try {
            $answer = Read-Host "$Prompt [$defaultHint]"
        }
        catch {
            Write-WarningNote "Could not read a response; using the default. $($_.Exception.Message)"
            return $Default
        }
        if ([string]::IsNullOrWhiteSpace($answer)) {
            return $Default
        }
        switch ($answer.Trim().ToLowerInvariant()) {
            'y' { return $true }
            'yes' { return $true }
            'n' { return $false }
            'no' { return $false }
            default { Write-WarningNote 'Please answer yes or no.' }
        }
    }
}

function Read-InstallerPort {
    param([int]$Default)

    while ($true) {
        try {
            $answer = Read-Host "io-workbench TCP port [$Default]"
        }
        catch {
            Write-WarningNote "Could not read a response; using port $Default. $($_.Exception.Message)"
            return $Default
        }
        if ([string]::IsNullOrWhiteSpace($answer)) {
            $candidate = $Default
        }
        else {
            try {
                $candidate = ConvertTo-InstallerPort -Value $answer -Name 'Port'
            }
            catch {
                Write-WarningNote $_.Exception.Message
                continue
            }
        }
        if (Test-InstallerPortInUse -Port $candidate) {
            if (-not (Read-InstallerYesNo -Prompt "Port $candidate appears to be in use. Keep it for an existing workbench upgrade?" -Default $false)) {
                continue
            }
        }
        return $candidate
    }
}

function Read-InstallerHost {
    param([System.Net.IPAddress]$Default)

    while ($true) {
        try {
            $answer = Read-Host "Bind address [$($Default.ToString())]"
        }
        catch {
            Write-WarningNote "Could not read a response; using $Default. $($_.Exception.Message)"
            return $Default
        }
        if ([string]::IsNullOrWhiteSpace($answer)) {
            $candidate = $Default
        }
        else {
            try {
                $candidate = ConvertTo-InstallerIPAddress -Value $answer
            }
            catch {
                Write-WarningNote $_.Exception.Message
                continue
            }
        }
        if (-not (Test-InstallerLoopbackAddress -Address $candidate)) {
            Write-WarningNote 'A non-loopback listener lets other machines contact this host.'
            Write-WarningNote 'Use HTTPS/WSS behind a trusted VPN, authenticated tunnel, or reverse proxy before remote use.'
            if (-not (Read-InstallerYesNo -Prompt 'Continue with this network-exposed bind address?' -Default $false)) {
                continue
            }
        }
        return $candidate
    }
}

function Read-InstallerDirectory {
    param(
        [string]$Prompt,
        [string]$Default,
        [bool]$MustExist,
        [string]$UserProfile
    )

    while ($true) {
        try {
            $answer = Read-Host "$Prompt [$Default]"
        }
        catch {
            Write-WarningNote "Could not read a response; using $Default. $($_.Exception.Message)"
            $answer = $Default
        }
        if ([string]::IsNullOrWhiteSpace($answer)) {
            $answer = $Default
        }
        try {
            $candidate = Resolve-InstallerPath -Value $answer -Name $Prompt -UserProfile $UserProfile
        }
        catch {
            Write-WarningNote $_.Exception.Message
            continue
        }
        if ((Test-Path -LiteralPath $candidate) -and -not (Test-Path -LiteralPath $candidate -PathType Container)) {
            Write-WarningNote 'That path exists but is not a directory.'
            continue
        }
        if ($MustExist -and -not (Test-Path -LiteralPath $candidate -PathType Container)) {
            Write-WarningNote 'Choose an existing directory. It defines files the Web UI may browse.'
            continue
        }
        return $candidate
    }
}

function Test-InstallerRootDirectory {
    param([string]$Path)
    $root = [System.IO.Path]::GetPathRoot($Path)
    if ([string]::IsNullOrWhiteSpace($root)) {
        return $false
    }
    return [string]::Equals(
        $Path.TrimEnd([char]92, [char]47),
        $root.TrimEnd([char]92, [char]47),
        [System.StringComparison]::OrdinalIgnoreCase
    )
}

function Add-DirectoryToUserPath {
    param([string]$Directory)

    $normalizedDirectory = [System.IO.Path]::GetFullPath($Directory).TrimEnd([char]92, [char]47)
    $userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
    $pathContainsDirectory = $false
    foreach ($pathEntry in @($userPath -split ';')) {
        if ([string]::IsNullOrWhiteSpace($pathEntry)) {
            continue
        }
        try {
            $normalizedEntry = [System.IO.Path]::GetFullPath([Environment]::ExpandEnvironmentVariables($pathEntry)).TrimEnd([char]92, [char]47)
        }
        catch {
            $normalizedEntry = $pathEntry.TrimEnd([char]92, [char]47)
        }
        if ([string]::Equals($normalizedEntry, $normalizedDirectory, [System.StringComparison]::OrdinalIgnoreCase)) {
            $pathContainsDirectory = $true
            break
        }
    }
    if (-not $pathContainsDirectory) {
        $updatedPath = if ([string]::IsNullOrWhiteSpace($userPath)) {
            $normalizedDirectory
        }
        else {
            "$userPath;$normalizedDirectory"
        }
        [Environment]::SetEnvironmentVariable('Path', $updatedPath, 'User')
        $env:Path = "$normalizedDirectory;$env:Path"
        return $true
    }
    return $false
}

function Get-CommandFilePath {
    param($Command)

    if ($null -eq $Command) {
        return $null
    }
    foreach ($propertyName in @('Path', 'Source', 'Definition')) {
        $property = $Command.PSObject.Properties[$propertyName]
        if ($null -eq $property -or [string]::IsNullOrWhiteSpace([string]$property.Value)) {
            continue
        }
        $candidate = [string]$property.Value
        if (Test-Path -LiteralPath $candidate -PathType Leaf) {
            return $candidate
        }
    }
    return $null
}

function Get-WorkbenchHealthUrl {
    param(
        [System.Net.IPAddress]$Address,
        [int]$Port
    )

    $hostValue = $Address.ToString()
    if ($hostValue -eq '0.0.0.0') {
        $hostValue = '127.0.0.1'
    }
    elseif ($hostValue -eq '::') {
        $hostValue = '::1'
    }
    if ($hostValue.Contains(':')) {
        return "http://[$hostValue]:$Port/health"
    }
    return "http://${hostValue}:$Port/health"
}

function Wait-ForWorkbenchHealth {
    param(
        [System.Net.IPAddress]$Address,
        [int]$Port
    )

    $url = Get-WorkbenchHealthUrl -Address $Address -Port $Port
    for ($attempt = 0; $attempt -lt 15; $attempt++) {
        try {
            $request = [System.Net.WebRequest]::Create($url)
            $request.Timeout = 2000
            $response = $request.GetResponse()
            try {
                if ([int]$response.StatusCode -ge 200 -and [int]$response.StatusCode -lt 500) {
                    return $true
                }
            }
            finally {
                $response.Dispose()
            }
        }
        catch {
            # The task may still be starting; retry shortly.
        }
        Start-Sleep -Seconds 1
    }
    return $false
}

function Get-InstallerTask {
    if (-not (Get-Command Get-ScheduledTask -ErrorAction SilentlyContinue)) {
        return $null
    }
    try {
        [array]$tasks = Get-ScheduledTask -TaskName $WorkbenchTaskName -TaskPath ([string][char]92) -ErrorAction SilentlyContinue
    }
    catch {
        return $null
    }
    if ($tasks.Count -ne 1) {
        return $null
    }
    if ([string]$tasks[0].Description -notlike "*$ManagedTaskMarker*") {
        return $null
    }
    return $tasks[0]
}

function Stop-InstallerTask {
    $task = Get-InstallerTask
    if ($null -eq $task -or -not (Get-Command Stop-ScheduledTask -ErrorAction SilentlyContinue)) {
        return
    }
    try {
        Stop-ScheduledTask -InputObject $task -ErrorAction Stop
        Start-Sleep -Milliseconds 500
    }
    catch {
        Write-WarningNote "Could not stop the existing io-workbench startup task: $($_.Exception.Message)"
    }
}

function Unregister-InstallerTask {
    $task = Get-InstallerTask
    if ($null -eq $task) {
        return $false
    }
    if (-not (Get-Command Unregister-ScheduledTask -ErrorAction SilentlyContinue)) {
        throw 'Windows Scheduled Tasks are unavailable, so the existing installer-managed task was left unchanged.'
    }
    Unregister-ScheduledTask -TaskName $task.TaskName -TaskPath $task.TaskPath -Confirm:$false -ErrorAction Stop
    return $true
}

function Write-WorkbenchLauncher {
    param(
        [string]$LauncherPath,
        [string]$WorkbenchPath,
        [System.Net.IPAddress]$Address,
        [int]$Port,
        [string]$ConfigDir,
        [string]$WorkspaceRoot,
        [string]$HomeDirectory,
        [string[]]$PathPrefixes
    )

    if (Test-Path -LiteralPath $LauncherPath -PathType Leaf) {
        $existing = Get-Content -LiteralPath $LauncherPath -Raw
        if ($existing -notlike "*$ManagedLauncherMarker*") {
            throw "Not overwriting an unrecognized launcher: $LauncherPath"
        }
    }
    elseif (Test-Path -LiteralPath $LauncherPath) {
        throw "Launcher path exists but is not a regular file: $LauncherPath"
    }

    $pathPrefix = ($PathPrefixes | Where-Object { -not [string]::IsNullOrWhiteSpace($_) } | Select-Object -Unique) -join [System.IO.Path]::PathSeparator
    $content = @(
        $ManagedLauncherMarker,
        '$ErrorActionPreference = ''Stop''',
        "`$env:HOME = $(ConvertTo-PowerShellLiteral -Value $HomeDirectory)",
        "`$env:IO_WORKBENCH_AUTH_REQUIRED = 'true'",
        "`$env:IO_WORKBENCH_HOST = $(ConvertTo-PowerShellLiteral -Value $Address.ToString())",
        "`$env:IO_WORKBENCH_PORT = $(ConvertTo-PowerShellLiteral -Value ([string]$Port))",
        "`$env:IO_WORKBENCH_CONFIG_DIR = $(ConvertTo-PowerShellLiteral -Value $ConfigDir)",
        "`$env:IO_WORKBENCH_WORKSPACE_ROOT = $(ConvertTo-PowerShellLiteral -Value $WorkspaceRoot)",
        "`$env:Path = $(ConvertTo-PowerShellLiteral -Value $pathPrefix) + [System.IO.Path]::PathSeparator + `$env:Path",
        "& $(ConvertTo-PowerShellLiteral -Value $WorkbenchPath) start",
        'exit $LASTEXITCODE',
        ''
    ) -join [Environment]::NewLine

    $temporaryPath = "$LauncherPath.install-$([Guid]::NewGuid().ToString('N'))"
    $utf8WithoutBom = New-Object System.Text.UTF8Encoding($false)
    [System.IO.File]::WriteAllText($temporaryPath, $content, $utf8WithoutBom)
    try {
        Copy-Item -LiteralPath $temporaryPath -Destination $LauncherPath -Force
    }
    finally {
        Remove-Item -LiteralPath $temporaryPath -Force -ErrorAction SilentlyContinue
    }
}

function Register-InstallerTask {
    param([string]$LauncherPath)

    $requiredCommands = @(
        'Register-ScheduledTask', 'Get-ScheduledTask', 'New-ScheduledTaskAction',
        'New-ScheduledTaskTrigger', 'New-ScheduledTaskSettingsSet', 'Start-ScheduledTask'
    )
    foreach ($commandName in $requiredCommands) {
        if (-not (Get-Command $commandName -ErrorAction SilentlyContinue)) {
            return $false
        }
    }

    [array]$existingTasks = Get-ScheduledTask -TaskName $WorkbenchTaskName -TaskPath ([string][char]92) -ErrorAction SilentlyContinue
    if ($existingTasks.Count -gt 0 -and $null -eq (Get-InstallerTask)) {
        throw "A Scheduled Task named '$WorkbenchTaskName' is not managed by this installer; it was left unchanged."
    }

    $powershellCommand = Get-Command powershell.exe -ErrorAction SilentlyContinue | Select-Object -First 1
    $powershellPath = Get-CommandFilePath -Command $powershellCommand
    if ([string]::IsNullOrWhiteSpace($powershellPath)) {
        throw 'Could not find powershell.exe to run the per-user startup task.'
    }
    $argument = "-NoProfile -ExecutionPolicy Bypass -File `"$LauncherPath`""
    $action = New-ScheduledTaskAction -Execute $powershellPath -Argument $argument
    $trigger = New-ScheduledTaskTrigger -AtLogOn
    $settings = New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries -StartWhenAvailable
    Register-ScheduledTask -TaskName $WorkbenchTaskName `
        -TaskPath ([string][char]92) `
        -Action $action `
        -Trigger $trigger `
        -Settings $settings `
        -Description "$ManagedTaskMarker. Starts the current user's authenticated io-workbench host at sign-in." `
        -Force | Out-Null
    Start-ScheduledTask -TaskName $WorkbenchTaskName -TaskPath ([string][char]92)
    return $true
}

function Install-IoGateway {
    param([string]$TemporaryDirectory)

    $installer = Join-Path $TemporaryDirectory 'io-gateway-install.ps1'
    $uri = 'https://github.com/giofahreza/io-gateway/releases/latest/download/install.ps1'
    Write-Note 'Downloading the optional IO Gateway installer.'
    try {
        Invoke-WebRequest -Uri $uri -OutFile $installer -Headers @{ 'User-Agent' = 'io-workbench-installer' } -UseBasicParsing
        $previousInteractive = $env:IO_GATEWAY_INTERACTIVE
        $previousAutoStart = $env:IO_GATEWAY_AUTOSTART
        try {
            # IO Gateway owns its own config and credentials. Keep it stopped
            # so the user can finish that separate setup deliberately.
            $env:IO_GATEWAY_INTERACTIVE = 'no'
            $env:IO_GATEWAY_AUTOSTART = 'no'
            $global:LASTEXITCODE = 0
            # The currently published gateway installer guarantees -NoStart;
            # do not depend on newer optional flags here.
            & $installer -NoStart
            if ($LASTEXITCODE -ne 0) {
                throw "IO Gateway installer exited with code $LASTEXITCODE."
            }
        }
        finally {
            $env:IO_GATEWAY_INTERACTIVE = $previousInteractive
            $env:IO_GATEWAY_AUTOSTART = $previousAutoStart
        }
    }
    catch {
        Write-WarningNote "Optional IO Gateway installation failed: $($_.Exception.Message)"
        return $false
    }
    return $true
}

function Install-ProviderCli {
    param(
        [string]$Label,
        [string]$Package,
        [string]$NpmPrefix
    )

    $npmCommand = Get-Command npm -ErrorAction SilentlyContinue | Select-Object -First 1
    $npmPath = Get-CommandFilePath -Command $npmCommand
    if ([string]::IsNullOrWhiteSpace($npmPath)) {
        Write-WarningNote "Cannot install ${Label}: npm and Node.js are not available. Install Node.js using your approved Windows method, then rerun with the matching option."
        return $false
    }
    try {
        New-Item -ItemType Directory -Path $NpmPrefix -Force | Out-Null
        Write-Note "Installing/updating $Label in $NpmPrefix."
        $global:LASTEXITCODE = 0
        & $npmPath install --global --prefix $NpmPrefix $Package
        if ($LASTEXITCODE -ne 0) {
            throw "npm exited with code $LASTEXITCODE."
        }
    }
    catch {
        Write-WarningNote "$Label installation failed: $($_.Exception.Message)"
        return $false
    }
    return $true
}

function Find-ProviderCli {
    param(
        [string]$CommandName,
        [string]$NpmPrefix
    )

    foreach ($candidate in @(
        (Join-Path $NpmPrefix "$CommandName.cmd"),
        (Join-Path $NpmPrefix "$CommandName.exe"),
        (Join-Path $NpmPrefix "$CommandName.ps1")
    )) {
        if (Test-Path -LiteralPath $candidate -PathType Leaf) {
            return $candidate
        }
    }
    $command = Get-Command $CommandName -ErrorAction SilentlyContinue | Select-Object -First 1
    $commandPath = Get-CommandFilePath -Command $command
    if (-not [string]::IsNullOrWhiteSpace($commandPath)) {
        return $commandPath
    }
    return $null
}

function Invoke-ProviderSetup {
    param(
        [string]$CommandName,
        [string]$Label,
        [string]$NpmPrefix
    )

    $provider = Find-ProviderCli -CommandName $CommandName -NpmPrefix $NpmPrefix
    if ([string]::IsNullOrWhiteSpace($provider)) {
        Write-WarningNote "$Label is unavailable, so its sign-in flow was skipped."
        return $false
    }
    Write-Note "Opening the native $Label sign-in/setup flow. Complete it, then return here."
    try {
        $global:LASTEXITCODE = 0
        switch ($CommandName) {
            'codex' { & $provider login }
            'claude' { & $provider auth login }
            'gemini' { & $provider }
            default { throw "Unknown provider command: $CommandName" }
        }
        if ($LASTEXITCODE -ne 0) {
            throw "$Label exited with code $LASTEXITCODE."
        }
    }
    catch {
        Write-WarningNote "$Label setup was not completed: $($_.Exception.Message)"
        return $false
    }
    return $true
}

function Invoke-ProviderCommandCapture {
    param(
        [string]$Provider,
        [string[]]$Arguments
    )

    try {
        $global:LASTEXITCODE = 0
        $output = (& $Provider @Arguments 2>&1 | Out-String)
        return [pscustomobject]@{
            ExitCode = [int]$LASTEXITCODE
            Output = $output
        }
    }
    catch {
        return [pscustomobject]@{
            ExitCode = 1
            Output = $_.Exception.Message
        }
    }
}

function Get-FirstProviderOutputLine {
    param([string]$Output)

    $line = @($Output -split "`r?`n" | Where-Object { -not [string]::IsNullOrWhiteSpace($_) } | Select-Object -First 1)
    if ($line.Count -eq 0) {
        return $null
    }
    return $line[0].Trim()
}

function Invoke-ProviderReadinessCheck {
    param(
        [string]$CommandName,
        [string]$Label,
        [string]$NpmPrefix,
        [string]$UserProfile
    )

    $provider = Find-ProviderCli -CommandName $CommandName -NpmPrefix $NpmPrefix
    if ([string]::IsNullOrWhiteSpace($provider)) {
        Write-Note "${Label}: not found; install it or add it to PATH before using that provider."
        return
    }

    $version = Invoke-ProviderCommandCapture -Provider $provider -Arguments @('--version')
    if ($version.ExitCode -eq 0) {
        $versionLine = Get-FirstProviderOutputLine -Output $version.Output
        if ([string]::IsNullOrWhiteSpace($versionLine)) {
            Write-Note "${Label}: version check passed."
        }
        else {
            Write-Note "${Label}: version check passed ($versionLine)."
        }
    }
    else {
        Write-WarningNote "${Label}: its version command failed; update or repair the CLI before using it in io-workbench."
    }

    switch ($CommandName) {
        'codex' {
            $auth = Invoke-ProviderCommandCapture -Provider $provider -Arguments @('login', 'status')
            if ($auth.ExitCode -eq 0) {
                Write-Note "${Label}: native login status reports authenticated."
            }
            else {
                Write-WarningNote "${Label}: native login status did not confirm authentication. Run: codex login"
            }
        }
        'claude' {
            $auth = Invoke-ProviderCommandCapture -Provider $provider -Arguments @('auth', 'status', '--json')
            if ($auth.ExitCode -eq 0 -and $auth.Output -match '"loggedIn"\s*:\s*true') {
                Write-Note "${Label}: native auth status reports authenticated."
            }
            else {
                Write-WarningNote "${Label}: native auth status did not confirm authentication. Run: claude auth login"
            }
        }
        'gemini' {
            $credentialHint = (
                -not [string]::IsNullOrWhiteSpace($env:GEMINI_API_KEY) -or
                -not [string]::IsNullOrWhiteSpace($env:GOOGLE_API_KEY) -or
                -not [string]::IsNullOrWhiteSpace($env:GOOGLE_APPLICATION_CREDENTIALS) -or
                (Test-Path -LiteralPath (Join-Path $UserProfile '.gemini\oauth_creds.json') -PathType Leaf)
            )
            if ($credentialHint) {
                Write-Note "${Label}: possible credential configuration detected (not read)."
            }
            else {
                Write-Note "${Label}: no portable offline auth-status command is available; native setup or a live test confirms access."
            }
        }
    }
}

function Invoke-ProviderReadinessChecks {
    param(
        [string]$NpmPrefix,
        [string]$UserProfile
    )

    Write-Note 'Checking available provider CLIs locally (version and native auth status only; no model request).'
    Invoke-ProviderReadinessCheck -CommandName 'codex' -Label 'Codex CLI' -NpmPrefix $NpmPrefix -UserProfile $UserProfile
    Invoke-ProviderReadinessCheck -CommandName 'claude' -Label 'Claude Code' -NpmPrefix $NpmPrefix -UserProfile $UserProfile
    Invoke-ProviderReadinessCheck -CommandName 'gemini' -Label 'Gemini CLI' -NpmPrefix $NpmPrefix -UserProfile $UserProfile
}

function Invoke-ProviderLiveCommand {
    param(
        [string]$Provider,
        [string]$ArgumentLine,
        [string]$WorkingDirectory,
        [string]$OutputDirectory,
        [string]$CommandName
    )

    $stdoutPath = Join-Path $OutputDirectory "provider-$CommandName-live.stdout"
    $stderrPath = Join-Path $OutputDirectory "provider-$CommandName-live.stderr"
    try {
        $process = Start-Process -FilePath $Provider -ArgumentList $ArgumentLine -WorkingDirectory $WorkingDirectory `
            -PassThru -NoNewWindow -RedirectStandardOutput $stdoutPath -RedirectStandardError $stderrPath
        if (-not $process.WaitForExit(90000)) {
            $taskKill = Get-Command taskkill.exe -ErrorAction SilentlyContinue | Select-Object -First 1
            $taskKillPath = Get-CommandFilePath -Command $taskKill
            if (-not [string]::IsNullOrWhiteSpace($taskKillPath)) {
                & $taskKillPath /PID $process.Id /T /F *> $null
            }
            else {
                Stop-Process -Id $process.Id -Force -ErrorAction SilentlyContinue
            }
            return [pscustomobject]@{ ExitCode = 124; Output = '' }
        }
        $output = ''
        foreach ($path in @($stdoutPath, $stderrPath)) {
            if (Test-Path -LiteralPath $path -PathType Leaf) {
                $output += (Get-Content -LiteralPath $path -Raw -ErrorAction SilentlyContinue) + "`n"
            }
        }
        return [pscustomobject]@{ ExitCode = [int]$process.ExitCode; Output = $output }
    }
    catch {
        return [pscustomobject]@{ ExitCode = 1; Output = $_.Exception.Message }
    }
}

function Invoke-ProviderLiveTest {
    param(
        [string]$CommandName,
        [string]$Label,
        [string]$NpmPrefix,
        [string]$TemporaryDirectory
    )

    $provider = Find-ProviderCli -CommandName $CommandName -NpmPrefix $NpmPrefix
    if ([string]::IsNullOrWhiteSpace($provider)) {
        Write-Note "$Label live test: skipped because the CLI is not installed."
        return
    }
    $probeDirectory = Join-Path $TemporaryDirectory 'provider-readiness-workspace'
    New-Item -ItemType Directory -Path $probeDirectory -Force | Out-Null
    $prompt = 'Reply with exactly READY and nothing else. Do not use tools, read files, execute commands, or change anything.'
    switch ($CommandName) {
        'codex' {
            $argumentLine = 'exec --ephemeral --sandbox read-only --skip-git-repo-check --ignore-rules --color never "' + $prompt + '"'
        }
        'claude' {
            $argumentLine = '--print --no-session-persistence --permission-mode plan --tools "" --no-chrome --strict-mcp-config "' + $prompt + '"'
        }
        'gemini' {
            $argumentLine = '--prompt "' + $prompt + '" --approval-mode plan --sandbox --extensions none --allowed-mcp-server-names none --output-format json --skip-trust'
        }
        default {
            Write-WarningNote "$Label live test: unsupported provider command."
            return
        }
    }

    Write-Note "$Label live test: sending one minimal provider request (read-only; may use quota or incur cost)."
    $result = Invoke-ProviderLiveCommand -Provider $provider -ArgumentLine $argumentLine -WorkingDirectory $probeDirectory `
        -OutputDirectory $TemporaryDirectory -CommandName $CommandName
    if ($result.ExitCode -eq 0 -and $result.Output -match '(?i)(^|[^a-z0-9_])READY([^a-z0-9_]|$)') {
        Write-Note "$Label live test: passed."
    }
    elseif ($result.Output -match '(?i)unknown (option|argument|flag)|unrecognized (option|argument|flag)|invalid (option|argument|flag)') {
        Write-WarningNote "$Label live test: the installed CLI rejected a safe-test flag. Update the CLI, then retry -TestProviders."
    }
    elseif ($result.ExitCode -eq 124) {
        Write-WarningNote "$Label live test: timed out after 90 seconds. Check provider connectivity and authentication, then retry -TestProviders."
    }
    elseif ($result.ExitCode -eq 0) {
        Write-WarningNote "$Label live test: the request finished but did not return the expected READY response. Retry it before relying on the provider."
    }
    else {
        Write-WarningNote "$Label live test: failed with exit code $($result.ExitCode). Check its native login/status command, then retry -TestProviders."
    }
}

function Invoke-ProviderLiveTests {
    param(
        [string]$NpmPrefix,
        [string]$TemporaryDirectory
    )

    Write-Note 'Running explicitly requested live provider readiness tests in a temporary empty workspace.'
    Invoke-ProviderLiveTest -CommandName 'codex' -Label 'Codex CLI' -NpmPrefix $NpmPrefix -TemporaryDirectory $TemporaryDirectory
    Invoke-ProviderLiveTest -CommandName 'claude' -Label 'Claude Code' -NpmPrefix $NpmPrefix -TemporaryDirectory $TemporaryDirectory
    Invoke-ProviderLiveTest -CommandName 'gemini' -Label 'Gemini CLI' -NpmPrefix $NpmPrefix -TemporaryDirectory $TemporaryDirectory
}

$portExplicit = $PSBoundParameters.ContainsKey('Port') -or -not [string]::IsNullOrWhiteSpace($env:IO_WORKBENCH_PORT)
$hostExplicit = $PSBoundParameters.ContainsKey('BindHost') -or -not [string]::IsNullOrWhiteSpace($env:IO_WORKBENCH_HOST)
$configDirExplicit = $PSBoundParameters.ContainsKey('ConfigDir') -or -not [string]::IsNullOrWhiteSpace($env:IO_WORKBENCH_CONFIG_DIR)
$workspaceRootExplicit = $PSBoundParameters.ContainsKey('WorkspaceRoot') -or -not [string]::IsNullOrWhiteSpace($env:IO_WORKBENCH_WORKSPACE_ROOT)

if ($RemainingArguments) {
    for ($argumentIndex = 0; $argumentIndex -lt $RemainingArguments.Count; $argumentIndex++) {
        $argument = $RemainingArguments[$argumentIndex]
        switch -Regex ($argument) {
            '^(--version|--Version)$' {
                $Version = Get-RemainingOptionValue -Arguments $RemainingArguments -Index ([ref]$argumentIndex) -OptionName '--version' -ValueDescription 'a release tag'
                continue
            }
            '^--version=(.+)$' { $Version = $Matches[1]; continue }
            '^(--host|--Host)$' {
                $BindHost = Get-RemainingOptionValue -Arguments $RemainingArguments -Index ([ref]$argumentIndex) -OptionName '--host' -ValueDescription 'an IP address'
                $hostExplicit = $true
                continue
            }
            '^--host=(.+)$' { $BindHost = $Matches[1]; $hostExplicit = $true; continue }
            '^(--port|--Port)$' {
                $Port = Get-RemainingOptionValue -Arguments $RemainingArguments -Index ([ref]$argumentIndex) -OptionName '--port' -ValueDescription 'a TCP port number'
                $portExplicit = $true
                continue
            }
            '^--port=(.+)$' { $Port = $Matches[1]; $portExplicit = $true; continue }
            '^(--config-dir|--ConfigDir)$' {
                $ConfigDir = Get-RemainingOptionValue -Arguments $RemainingArguments -Index ([ref]$argumentIndex) -OptionName '--config-dir' -ValueDescription 'a directory path'
                $configDirExplicit = $true
                continue
            }
            '^--config-dir=(.+)$' { $ConfigDir = $Matches[1]; $configDirExplicit = $true; continue }
            '^(--workspace-root|--WorkspaceRoot)$' {
                $WorkspaceRoot = Get-RemainingOptionValue -Arguments $RemainingArguments -Index ([ref]$argumentIndex) -OptionName '--workspace-root' -ValueDescription 'an existing directory path'
                $workspaceRootExplicit = $true
                continue
            }
            '^--workspace-root=(.+)$' { $WorkspaceRoot = $Matches[1]; $workspaceRootExplicit = $true; continue }
            '^(--autostart|--auto-start|--start)$' { $AutoStart = $true; continue }
            '^(--no-autostart|--no-auto-start|--no-start)$' { $NoAutoStart = $true; $NoStart = $true; continue }
            '^(--with-io-gateway|--install-io-gateway)$' { $InstallIoGateway = $true; continue }
            '^(--without-io-gateway|--no-io-gateway)$' { $NoIoGateway = $true; continue }
            '^(--with-codex|--install-codex)$' { $InstallCodex = $true; continue }
            '^(--without-codex|--no-codex)$' { $NoCodex = $true; continue }
            '^(--with-claude|--install-claude)$' { $InstallClaude = $true; continue }
            '^(--without-claude|--no-claude)$' { $NoClaude = $true; continue }
            '^(--with-gemini|--install-gemini)$' { $InstallGemini = $true; continue }
            '^(--without-gemini|--no-gemini)$' { $NoGemini = $true; continue }
            '^(--configure-clis)$' { $ConfigureClis = $true; continue }
            '^(--no-configure-clis)$' { $NoConfigureClis = $true; continue }
            '^(--check-providers)$' { $CheckProviders = $true; continue }
            '^(--no-check-providers)$' { $NoCheckProviders = $true; continue }
            '^(--test-providers)$' { $TestProviders = $true; continue }
            '^(--no-test-providers)$' { $NoTestProviders = $true; continue }
            '^(--interactive)$' { $Interactive = $true; continue }
            '^(--non-interactive)$' { $NonInteractive = $true; continue }
            '^(--help|--Help|-h|-Help)$' { Show-Usage; exit 0 }
            default { throw "Unknown option: $argument. Run with -Help for usage." }
        }
    }
}

if ($InstallIoGateway -and $NoIoGateway) { throw 'Choose only one of -InstallIoGateway and -NoIoGateway.' }
if ($InstallCodex -and $NoCodex) { throw 'Choose only one of -InstallCodex and -NoCodex.' }
if ($InstallClaude -and $NoClaude) { throw 'Choose only one of -InstallClaude and -NoClaude.' }
if ($InstallGemini -and $NoGemini) { throw 'Choose only one of -InstallGemini and -NoGemini.' }
if ($ConfigureClis -and $NoConfigureClis) { throw 'Choose only one of -ConfigureClis and -NoConfigureClis.' }
if ($CheckProviders -and $NoCheckProviders) { throw 'Choose only one of -CheckProviders and -NoCheckProviders.' }
if ($TestProviders -and $NoTestProviders) { throw 'Choose only one of -TestProviders and -NoTestProviders.' }
if ($AutoStart -and ($NoAutoStart -or $NoStart)) { throw 'Choose only one of -AutoStart and -NoAutoStart.' }
if ($Interactive -and $NonInteractive) { throw 'Choose only one of -Interactive and -NonInteractive.' }

function Get-ConfiguredMode {
    param(
        [string]$EnvironmentVariable,
        [string]$Default = 'auto'
    )
    $value = [Environment]::GetEnvironmentVariable($EnvironmentVariable)
    if ([string]::IsNullOrWhiteSpace($value)) {
        return $Default
    }
    return ConvertTo-InstallerMode -Value $value -Name $EnvironmentVariable
}

$autoStartMode = Get-ConfiguredMode -EnvironmentVariable 'IO_WORKBENCH_AUTOSTART'
$gatewayMode = Get-ConfiguredMode -EnvironmentVariable 'IO_WORKBENCH_INSTALL_IO_GATEWAY'
$codexMode = Get-ConfiguredMode -EnvironmentVariable 'IO_WORKBENCH_INSTALL_CODEX'
$claudeMode = Get-ConfiguredMode -EnvironmentVariable 'IO_WORKBENCH_INSTALL_CLAUDE'
$geminiMode = Get-ConfiguredMode -EnvironmentVariable 'IO_WORKBENCH_INSTALL_GEMINI'
$configureClisMode = Get-ConfiguredMode -EnvironmentVariable 'IO_WORKBENCH_CONFIGURE_CLIS'
$checkProvidersMode = Get-ConfiguredMode -EnvironmentVariable 'IO_WORKBENCH_CHECK_PROVIDERS'
$testProvidersMode = Get-ConfiguredMode -EnvironmentVariable 'IO_WORKBENCH_TEST_PROVIDERS'
$interactiveMode = Get-ConfiguredMode -EnvironmentVariable 'IO_WORKBENCH_INTERACTIVE'

if ($AutoStart) { $autoStartMode = 'yes' }
elseif ($NoAutoStart -or $NoStart) { $autoStartMode = 'no' }
if ($InstallIoGateway) { $gatewayMode = 'yes' }
elseif ($NoIoGateway) { $gatewayMode = 'no' }
if ($InstallCodex) { $codexMode = 'yes' }
elseif ($NoCodex) { $codexMode = 'no' }
if ($InstallClaude) { $claudeMode = 'yes' }
elseif ($NoClaude) { $claudeMode = 'no' }
if ($InstallGemini) { $geminiMode = 'yes' }
elseif ($NoGemini) { $geminiMode = 'no' }
if ($ConfigureClis) { $configureClisMode = 'yes' }
elseif ($NoConfigureClis) { $configureClisMode = 'no' }
if ($CheckProviders) { $checkProvidersMode = 'yes' }
elseif ($NoCheckProviders) { $checkProvidersMode = 'no' }
if ($TestProviders) { $testProvidersMode = 'yes' }
elseif ($NoTestProviders) { $testProvidersMode = 'no' }
if ($Interactive) { $interactiveMode = 'yes' }
elseif ($NonInteractive) { $interactiveMode = 'no' }

if ([Environment]::OSVersion.Platform -ne [System.PlatformID]::Win32NT) {
    throw 'This installer is for Windows. Use install.sh on Linux or macOS.'
}

try {
    [Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
}
catch {
    # Modern PowerShell has secure defaults and may not expose this legacy API.
}

if ([string]::IsNullOrWhiteSpace($Version)) { $Version = 'latest' }
if ([string]::IsNullOrWhiteSpace($Repository)) { $Repository = 'giofahreza/io-workbench' }
if ($Repository -notmatch '^[A-Za-z0-9._-]+/[A-Za-z0-9._-]+$') {
    throw 'Repository must be in owner/repository form.'
}

$userProfile = Get-UserProfilePath
$localAppData = $env:LOCALAPPDATA
if ([string]::IsNullOrWhiteSpace($localAppData)) {
    $localAppData = Join-Path $userProfile 'AppData\Local'
}
if ([string]::IsNullOrWhiteSpace($InstallDir)) { $InstallDir = Join-Path $localAppData 'Programs\io-workbench' }
if ([string]::IsNullOrWhiteSpace($BindHost)) { $BindHost = '127.0.0.1' }
if ([string]::IsNullOrWhiteSpace($Port)) { $Port = '8787' }
if ([string]::IsNullOrWhiteSpace($ConfigDir)) { $ConfigDir = Join-Path $userProfile '.io-workbench' }
if ([string]::IsNullOrWhiteSpace($WorkspaceRoot)) { $WorkspaceRoot = $userProfile }
if ([string]::IsNullOrWhiteSpace($NpmPrefix)) { $NpmPrefix = Join-Path $localAppData 'io-workbench\npm' }

$InstallDir = Resolve-InstallerPath -Value $InstallDir -Name 'Install directory' -UserProfile $userProfile
$NpmPrefix = Resolve-InstallerPath -Value $NpmPrefix -Name 'npm prefix' -UserProfile $userProfile
$selectedAddress = ConvertTo-InstallerIPAddress -Value $BindHost
$selectedPort = ConvertTo-InstallerPort -Value $Port

$interactiveSetup = $false
switch ($interactiveMode) {
    'yes' {
        $interactiveSetup = Test-InstallerInteractive -Disabled $false
        if (-not $interactiveSetup) {
            throw '-Interactive requires a controlling terminal. Use -NonInteractive for automation.'
        }
    }
    'no' { $interactiveSetup = $false }
    default { $interactiveSetup = Test-InstallerInteractive -Disabled $false }
}

if ($interactiveSetup) {
    Write-Host ''
    Write-Note 'First-run setup. Press Enter to accept a value shown in brackets.'
    Write-Host 'This host can run code, terminals, databases, and provider CLIs for the workspace you allow.'
    Write-Host 'Authentication stays enabled. This installer never requests a password, token, OTP secret, API key, or provider credential.'
    if (-not $portExplicit) {
        $selectedPort = Read-InstallerPort -Default $selectedPort
    }
    elseif (Test-InstallerPortInUse -Port $selectedPort) {
        if (-not (Read-InstallerYesNo -Prompt "Port $selectedPort appears to be in use. Keep it for an existing workbench upgrade?" -Default $false)) {
            throw 'Installation cancelled before downloading a release.'
        }
    }
    if (-not $hostExplicit) {
        $selectedAddress = Read-InstallerHost -Default $selectedAddress
    }
    elseif (-not (Test-InstallerLoopbackAddress -Address $selectedAddress)) {
        Write-WarningNote 'The selected bind address accepts network connections; secure it with HTTPS/WSS and a trusted boundary.'
        if (-not (Read-InstallerYesNo -Prompt 'Continue with this network-exposed bind address?' -Default $false)) {
            throw 'Installation cancelled before downloading a release.'
        }
    }
    if (-not $configDirExplicit) {
        $ConfigDir = Read-InstallerDirectory -Prompt 'Data/configuration directory' -Default $ConfigDir -MustExist $false -UserProfile $userProfile
    }
    if (-not $workspaceRootExplicit) {
        $WorkspaceRoot = Read-InstallerDirectory -Prompt 'Workspace root (existing directory)' -Default $WorkspaceRoot -MustExist $true -UserProfile $userProfile
    }
}

$ConfigDir = Resolve-InstallerPath -Value $ConfigDir -Name 'Data/configuration directory' -UserProfile $userProfile
$WorkspaceRoot = Resolve-InstallerPath -Value $WorkspaceRoot -Name 'Workspace root' -UserProfile $userProfile
if ((Test-Path -LiteralPath $ConfigDir) -and -not (Test-Path -LiteralPath $ConfigDir -PathType Container)) {
    throw "Data/configuration path exists but is not a directory: $ConfigDir"
}
if (-not (Test-Path -LiteralPath $WorkspaceRoot -PathType Container)) {
    throw "Workspace root does not exist or is not a directory: $WorkspaceRoot"
}
if (Test-InstallerRootDirectory -Path $WorkspaceRoot) {
    if ($interactiveSetup) {
        Write-WarningNote 'A workspace root at a drive or share root grants the Web UI authority over every accessible file on that volume.'
        if (-not (Read-InstallerYesNo -Prompt "Keep $WorkspaceRoot as the workspace root?" -Default $false)) {
            throw 'Installation cancelled before downloading a release.'
        }
    }
    else {
        Write-WarningNote "Workspace root $WorkspaceRoot grants the Web UI broad authority over that volume."
    }
}
if (-not (Test-InstallerLoopbackAddress -Address $selectedAddress)) {
    Write-WarningNote "io-workbench will bind to $($selectedAddress.ToString()). Authentication remains enabled, but remote use still needs an HTTPS/WSS network boundary."
}
if (-not $interactiveSetup -and (Test-InstallerPortInUse -Port $selectedPort)) {
    Write-WarningNote "Port $selectedPort appears to be in use. It may be an existing workbench upgrade or the new host may fail to start."
}

$hadManagedTask = $false
try {
    $hadManagedTask = $null -ne (Get-InstallerTask)
}
catch {
    $hadManagedTask = $false
}

$enableAutoStart = $false
$installGateway = $false
$installCodex = $false
$installClaude = $false
$installGemini = $false
$configureProviderClis = $false
$checkProviderClis = $false
$testProviderClis = $false
if ($interactiveSetup) {
    switch ($autoStartMode) {
        'yes' { $enableAutoStart = $true }
        'no' { $enableAutoStart = $false }
        default { $enableAutoStart = Read-InstallerYesNo -Prompt 'Enable and start a per-user io-workbench task at Windows sign-in?' -Default $hadManagedTask }
    }
    switch ($gatewayMode) {
        'yes' { $installGateway = $true }
        'no' { $installGateway = $false }
        default { $installGateway = Read-InstallerYesNo -Prompt 'Install the optional IO Gateway (separate localhost-only setup)?' -Default $false }
    }
    switch ($codexMode) {
        'yes' { $installCodex = $true }
        'no' { $installCodex = $false }
        default { $installCodex = Read-InstallerYesNo -Prompt 'Install/update the Codex CLI with npm?' -Default $false }
    }
    switch ($claudeMode) {
        'yes' { $installClaude = $true }
        'no' { $installClaude = $false }
        default { $installClaude = Read-InstallerYesNo -Prompt 'Install/update Claude Code with npm?' -Default $false }
    }
    switch ($geminiMode) {
        'yes' { $installGemini = $true }
        'no' { $installGemini = $false }
        default { $installGemini = Read-InstallerYesNo -Prompt 'Install/update Gemini CLI with npm?' -Default $false }
    }
    switch ($configureClisMode) {
        'yes' { $configureProviderClis = $true }
        'no' { $configureProviderClis = $false }
        default {
            $hasAvailableProviderCli = (
                $installCodex -or
                $installClaude -or
                $installGemini -or
                (-not [string]::IsNullOrWhiteSpace((Find-ProviderCli -CommandName 'codex' -NpmPrefix $NpmPrefix))) -or
                (-not [string]::IsNullOrWhiteSpace((Find-ProviderCli -CommandName 'claude' -NpmPrefix $NpmPrefix))) -or
                (-not [string]::IsNullOrWhiteSpace((Find-ProviderCli -CommandName 'gemini' -NpmPrefix $NpmPrefix)))
            )
            if ($hasAvailableProviderCli) {
                Write-Host 'Native provider setup can open a browser or interactive session; credentials stay with that provider CLI.'
                $configureProviderClis = Read-InstallerYesNo -Prompt 'Open available provider login/setup flows after installation?' -Default $false
            }
        }
    }

    $hasSelectedOrAvailableProviderCli = (
        $installCodex -or
        $installClaude -or
        $installGemini -or
        (-not [string]::IsNullOrWhiteSpace((Find-ProviderCli -CommandName 'codex' -NpmPrefix $NpmPrefix))) -or
        (-not [string]::IsNullOrWhiteSpace((Find-ProviderCli -CommandName 'claude' -NpmPrefix $NpmPrefix))) -or
        (-not [string]::IsNullOrWhiteSpace((Find-ProviderCli -CommandName 'gemini' -NpmPrefix $NpmPrefix)))
    )
    switch ($checkProvidersMode) {
        'yes' { $checkProviderClis = $true }
        'no' { $checkProviderClis = $false }
        default {
            if ($hasSelectedOrAvailableProviderCli) {
                Write-Host 'Local readiness checks run version and native authentication-status commands only; they do not send a model request.'
                $checkProviderClis = Read-InstallerYesNo -Prompt 'Check available provider CLI readiness after installation?' -Default $true
            }
        }
    }
    switch ($testProvidersMode) {
        'yes' { $testProviderClis = $true }
        'no' { $testProviderClis = $false }
        default {
            if ($hasSelectedOrAvailableProviderCli) {
                Write-Host 'A live test sends one tiny request to every available provider. It uses a temporary empty workspace and safe read-only/no-tool modes, but may use provider quota or incur cost.'
                $testProviderClis = Read-InstallerYesNo -Prompt 'Run live provider readiness tests after installation?' -Default $false
            }
        }
    }

    Write-Host ''
    Write-Host 'Installation summary'
    Write-Host "  Bind: $($selectedAddress.ToString()):$selectedPort"
    Write-Host "  Config: $ConfigDir"
    Write-Host "  Workspace authority: $WorkspaceRoot"
    if ($enableAutoStart) { Write-Host '  Startup: per-user Windows Scheduled Task' } else { Write-Host '  Startup: manual' }
    if ($installGateway) { Write-Host '  IO Gateway: install only (no auto-start)' }
    if ($installCodex) { Write-Host '  Codex CLI: npm install/update' }
    if ($installClaude) { Write-Host '  Claude Code: npm install/update' }
    if ($installGemini) { Write-Host '  Gemini CLI: npm install/update' }
    if ($configureProviderClis) { Write-Host '  Provider login: native flow(s) for available CLIs after installation' }
    if ($checkProviderClis) { Write-Host '  Provider readiness: local version/auth checks (no model request)' }
    if ($testProviderClis) { Write-Host '  Provider readiness: one live read-only request per available CLI (may use quota/cost)' }
    if (-not (Read-InstallerYesNo -Prompt 'Download and apply this setup?' -Default $true)) {
        throw 'Installation cancelled before downloading a release.'
    }
}
else {
    $enableAutoStart = if ($autoStartMode -eq 'yes') { $true } elseif ($autoStartMode -eq 'auto') { $hadManagedTask } else { $false }
    $installGateway = $gatewayMode -eq 'yes'
    $installCodex = $codexMode -eq 'yes'
    $installClaude = $claudeMode -eq 'yes'
    $installGemini = $geminiMode -eq 'yes'
    $configureProviderClis = $configureClisMode -eq 'yes'
    $checkProviderClis = $checkProvidersMode -eq 'yes'
    $testProviderClis = $testProvidersMode -eq 'yes'
    if ($configureProviderClis) {
        throw '-ConfigureClis requires a controlling terminal. Use -Interactive to complete native provider login.'
    }
    Write-Note 'No interactive terminal detected; using safe defaults unless explicit options selected more.'
}

$architecture = [System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture.ToString()
switch ($architecture) {
    'X64' { $target = 'windows-x86_64' }
    'Arm64' { $target = 'windows-aarch64' }
    default { throw "Unsupported Windows CPU architecture: $architecture." }
}

if ($Version -eq 'latest') {
    Write-Note "Resolving the latest release from $Repository."
    $release = Invoke-RestMethod -Uri "https://api.github.com/repos/$Repository/releases/latest" -Headers @{ 'User-Agent' = 'io-workbench-installer' }
    $tag = [string]$release.tag_name
    if ([string]::IsNullOrWhiteSpace($tag)) {
        throw 'Could not read tag_name from the GitHub release response.'
    }
}
elseif ($Version -match '^v[0-9][A-Za-z0-9._-]*$') {
    $tag = $Version
}
elseif ($Version -match '^[0-9][A-Za-z0-9._-]*$') {
    $tag = "v$Version"
}
else {
    throw "Invalid release version: $Version"
}

if ($tag -notmatch '^v[0-9][A-Za-z0-9._-]*$') {
    throw "Invalid GitHub release tag: $tag"
}

$assetName = "io-workbench-$tag-$target.zip"
$releaseBase = "https://github.com/$Repository/releases/download/$tag"
$tempDir = Join-Path ([IO.Path]::GetTempPath()) ("io-workbench-install-" + [Guid]::NewGuid().ToString('N'))
$archive = Join-Path $tempDir $assetName
$sumsFile = Join-Path $tempDir 'SHA256SUMS'
$extractDir = Join-Path $tempDir 'package'
$workbenchPath = Join-Path $InstallDir 'io-workbench.exe'
$iowbPath = Join-Path $InstallDir 'iowb.exe'
$launcherPath = Join-Path $ConfigDir 'io-workbench-start.ps1'
$taskRegistered = $false
$taskStarted = $false
$gatewayInstalled = $false
$pathAdded = $false
$npmPathAdded = $false

try {
    New-Item -ItemType Directory -Path $tempDir -Force | Out-Null
    Write-Note "Downloading $assetName."
    Invoke-WebRequest -Uri "$releaseBase/$assetName" -OutFile $archive -Headers @{ 'User-Agent' = 'io-workbench-installer' } -UseBasicParsing
    Invoke-WebRequest -Uri "$releaseBase/SHA256SUMS" -OutFile $sumsFile -Headers @{ 'User-Agent' = 'io-workbench-installer' } -UseBasicParsing

    $sumLine = Get-Content -LiteralPath $sumsFile | Where-Object {
        $_ -match ('^([0-9A-Fa-f]{64})\s+\*?' + [regex]::Escape($assetName) + '$')
    } | Select-Object -First 1
    if ([string]::IsNullOrWhiteSpace($sumLine)) {
        throw "SHA256SUMS does not contain $assetName."
    }
    $expectedSha256 = (($sumLine -split '\s+')[0]).ToLowerInvariant()
    $actualSha256 = (Get-FileHash -LiteralPath $archive -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($actualSha256 -ne $expectedSha256) {
        throw "Checksum verification failed for $assetName."
    }
    Write-Note 'Release checksum verified.'

    Expand-Archive -LiteralPath $archive -DestinationPath $extractDir -Force
    foreach ($requiredFile in @('io-workbench.exe', 'iowb.exe')) {
        if (-not (Test-Path -LiteralPath (Join-Path $extractDir $requiredFile) -PathType Leaf)) {
            throw "Release archive is missing required file: $requiredFile"
        }
    }

    if ($hadManagedTask) {
        Stop-InstallerTask
    }
    New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null
    Copy-Item -LiteralPath (Join-Path $extractDir 'io-workbench.exe') -Destination $workbenchPath -Force
    Copy-Item -LiteralPath (Join-Path $extractDir 'iowb.exe') -Destination $iowbPath -Force
    $pathAdded = Add-DirectoryToUserPath -Directory $InstallDir
    Write-Note "Installed io-workbench.exe and iowb.exe to $InstallDir."

    if ($installGateway) {
        $gatewayInstalled = Install-IoGateway -TemporaryDirectory $tempDir
    }
    if ($installCodex) {
        if (Install-ProviderCli -Label 'Codex CLI' -Package '@openai/codex' -NpmPrefix $NpmPrefix) {
            $npmPathAdded = (Add-DirectoryToUserPath -Directory $NpmPrefix) -or $npmPathAdded
        }
    }
    if ($installClaude) {
        if (Install-ProviderCli -Label 'Claude Code' -Package '@anthropic-ai/claude-code' -NpmPrefix $NpmPrefix) {
            $npmPathAdded = (Add-DirectoryToUserPath -Directory $NpmPrefix) -or $npmPathAdded
        }
    }
    if ($installGemini) {
        if (Install-ProviderCli -Label 'Gemini CLI' -Package '@google/gemini-cli' -NpmPrefix $NpmPrefix) {
            $npmPathAdded = (Add-DirectoryToUserPath -Directory $NpmPrefix) -or $npmPathAdded
        }
    }

    if ($autoStartMode -eq 'no') {
        try {
            if (Unregister-InstallerTask) {
                Write-Note 'Removed the installer-managed per-user io-workbench startup task.'
            }
        }
        catch {
            Write-WarningNote "Could not remove the per-user startup task: $($_.Exception.Message)"
        }
    }
    elseif ($enableAutoStart) {
        $nodeCommand = Get-Command node -ErrorAction SilentlyContinue | Select-Object -First 1
        $nodePath = Get-CommandFilePath -Command $nodeCommand
        $nodeDirectory = if (-not [string]::IsNullOrWhiteSpace($nodePath)) {
            Split-Path -Parent $nodePath
        }
        else {
            $null
        }
        try {
            New-Item -ItemType Directory -Path $ConfigDir -Force | Out-Null
            Write-WorkbenchLauncher -LauncherPath $launcherPath -WorkbenchPath $workbenchPath `
                -Address $selectedAddress -Port $selectedPort -ConfigDir $ConfigDir -WorkspaceRoot $WorkspaceRoot `
                -HomeDirectory $userProfile `
                -PathPrefixes @($NpmPrefix, $InstallDir, $nodeDirectory)
            $taskRegistered = Register-InstallerTask -LauncherPath $launcherPath
            if ($taskRegistered) {
                $taskStarted = Wait-ForWorkbenchHealth -Address $selectedAddress -Port $selectedPort
                if ($taskStarted) {
                    Write-Note "Started the per-user task at $(Get-WorkbenchHealthUrl -Address $selectedAddress -Port $selectedPort)."
                }
                else {
                    Write-WarningNote 'The per-user task was registered, but it did not become healthy within 15 seconds. Inspect it with Get-ScheduledTask -TaskName io-workbench.'
                }
            }
            else {
                Write-WarningNote 'Windows Scheduled Tasks are unavailable; start io-workbench manually instead.'
            }
        }
        catch {
            Write-WarningNote "Could not register or start the per-user task: $($_.Exception.Message)"
        }
    }

    if ($configureProviderClis) {
        # Explicit setup also covers provider CLIs that were already present
        # before this run; the helper skips unavailable commands safely.
        Invoke-ProviderSetup -CommandName 'codex' -Label 'Codex CLI' -NpmPrefix $NpmPrefix | Out-Null
        Invoke-ProviderSetup -CommandName 'claude' -Label 'Claude Code' -NpmPrefix $NpmPrefix | Out-Null
        Invoke-ProviderSetup -CommandName 'gemini' -Label 'Gemini CLI' -NpmPrefix $NpmPrefix | Out-Null
    }
    if ($checkProviderClis) {
        try {
            Invoke-ProviderReadinessChecks -NpmPrefix $NpmPrefix -UserProfile $userProfile
        }
        catch {
            Write-WarningNote "Provider readiness checks could not complete: $($_.Exception.Message)"
        }
    }
    if ($testProviderClis) {
        try {
            Invoke-ProviderLiveTests -NpmPrefix $NpmPrefix -TemporaryDirectory $tempDir
        }
        catch {
            Write-WarningNote "Provider live tests could not complete: $($_.Exception.Message)"
        }
    }
}
finally {
    if (Test-Path -LiteralPath $tempDir) {
        Remove-Item -LiteralPath $tempDir -Recurse -Force
    }
}

Write-Host ''
Write-Note "Installed $tag for $target."
Write-Note "Runtime data: $ConfigDir"
Write-Note "Workspace authority: $WorkspaceRoot"
if ($pathAdded) {
    Write-Host 'Added the io-workbench install directory to your user PATH; open a new terminal to use its commands by name.'
}
if ($npmPathAdded) {
    Write-Host "Added $NpmPrefix to your user PATH for the selected provider CLIs; open a new terminal before invoking them by name."
}
if ($taskRegistered -and $taskStarted) {
    $openUrl = (Get-WorkbenchHealthUrl -Address $selectedAddress -Port $selectedPort) -replace '/health$', ''
    Write-Host "Open $openUrl and complete first-user setup."
}
elseif ($taskRegistered) {
    Write-Host 'The per-user task is registered for Windows sign-in. Start or inspect it with Get-ScheduledTask -TaskName io-workbench.'
}
else {
    $manualHost = $selectedAddress.ToString()
    Write-Host 'Start a local, authenticated workbench when you are ready:'
    Write-Host "  `$env:IO_WORKBENCH_AUTH_REQUIRED = 'true'"
    Write-Host "  `$env:IO_WORKBENCH_HOST = '$(($manualHost).Replace("'", "''"))'"
    Write-Host "  `$env:IO_WORKBENCH_PORT = '$selectedPort'"
    Write-Host "  `$env:IO_WORKBENCH_CONFIG_DIR = '$(($ConfigDir).Replace("'", "''"))'"
    Write-Host "  `$env:IO_WORKBENCH_WORKSPACE_ROOT = '$(($WorkspaceRoot).Replace("'", "''"))'"
    Write-Host "  & '$($workbenchPath.Replace("'", "''"))' start"
    $openUrl = (Get-WorkbenchHealthUrl -Address $selectedAddress -Port $selectedPort) -replace '/health$', ''
    Write-Host "Then open $openUrl and complete first-user setup."
}
Write-Host 'Authentication is enabled; this installer stored no password, token, OTP secret, API key, or provider credential.'
if ($gatewayInstalled) {
    Write-Host 'IO Gateway is separate and was not auto-started. Finish it, then use Settings → IO Gateway to enter its URL and proxy API key.'
}
if ($installCodex -or $installClaude -or $installGemini) {
    Write-Host "Selected provider CLIs install under $NpmPrefix. The per-user task, when enabled, includes that directory in PATH."
}
