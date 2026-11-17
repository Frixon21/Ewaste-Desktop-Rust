# Shared by build.ps1 and coverage.ps1. Inspect the current process environment;
# do not cache readiness in a flag or repeatedly append the developer-shell paths.
function ConvertTo-EwasteBuildPath {
    param([AllowNull()][string]$Value)
    if ([string]::IsNullOrWhiteSpace($Value)) { return $null }
    try {
        return [IO.Path]::GetFullPath($Value.Trim().Trim('"')).TrimEnd([char[]]@('\','/'))
    } catch { return $null }
}

function Test-EwasteBuildFileResolution {
    param([AllowNull()][string]$Value, [string]$FileName, [string]$Expected)
    $expectedPath = ConvertTo-EwasteBuildPath $Expected
    foreach ($entry in ($Value -split ';')) {
        # Cargo changes working directory. A relative/empty prefix can resolve
        # differently there, so do not reuse it before a verified absolute file.
        $entry = $entry.Trim().Trim('"')
        if ($entry -notmatch '^(?:[A-Za-z]:[\\/]|[\\/]{2})') { return $false }
        $directory = ConvertTo-EwasteBuildPath $entry
        if (!$directory) { continue }
        $candidate = Join-Path $directory $FileName
        if (Test-Path -LiteralPath $candidate -PathType Leaf) {
            return (ConvertTo-EwasteBuildPath $candidate) -eq $expectedPath
        }
    }
    return $false
}

function Test-EwasteVisualStudioEnvironment {
    param([string]$VsInstallPath)
    $installation = ConvertTo-EwasteBuildPath $VsInstallPath
    if (!$installation -or (ConvertTo-EwasteBuildPath $env:VSINSTALLDIR) -ne $installation -or
        $env:VSCMD_ARG_HOST_ARCH -ne 'x64' -or $env:VSCMD_ARG_TGT_ARCH -ne 'x64') {
        return $false
    }

    $tools = ConvertTo-EwasteBuildPath $env:VCToolsInstallDir
    $toolsParent = ConvertTo-EwasteBuildPath (Join-Path $installation 'VC/Tools/MSVC')
    if (!$tools -or [IO.Path]::GetDirectoryName($tools) -ne $toolsParent) { return $false }
    foreach ($name in @('cl.exe','link.exe')) {
        $command = Get-Command $name -CommandType Application -ErrorAction SilentlyContinue | Select-Object -First 1
        $expected = ConvertTo-EwasteBuildPath (Join-Path $tools "bin/Hostx64/x64/$name")
        if (!$command -or (ConvertTo-EwasteBuildPath $command.Path) -ne $expected -or
            !(Test-EwasteBuildFileResolution $env:PATH $name $expected)) { return $false }
    }

    # Binary paths and architecture flags must agree with usable x64 headers/libs.
    if (!(Test-EwasteBuildFileResolution $env:INCLUDE 'vcruntime.h' (Join-Path $tools 'include/vcruntime.h')) -or
        !(Test-EwasteBuildFileResolution $env:LIB 'vcruntime.lib' (Join-Path $tools 'lib/x64/vcruntime.lib'))) {
        return $false
    }
    $sdk = ConvertTo-EwasteBuildPath $env:WindowsSdkDir
    $sdkVersion = $env:WindowsSDKVersion
    if (!$sdk -or [string]::IsNullOrWhiteSpace($sdkVersion)) { return $false }
    $sdkVersion = $sdkVersion.Trim([char[]]@('\','/'))
    foreach ($header in @('ucrt/corecrt.h','shared/sdkddkver.h','um/windows.h')) {
        $expected = Join-Path $sdk "Include/$sdkVersion/$header"
        if (!(Test-EwasteBuildFileResolution $env:INCLUDE ([IO.Path]::GetFileName($expected)) $expected)) { return $false }
    }
    foreach ($library in @('ucrt/x64/ucrt.lib','um/x64/kernel32.lib')) {
        $expected = Join-Path $sdk "Lib/$sdkVersion/$library"
        if (!(Test-EwasteBuildFileResolution $env:LIB ([IO.Path]::GetFileName($expected)) $expected)) { return $false }
    }
    return $true
}

function Initialize-EwasteVisualStudioEnvironment {
    param([Parameter(Mandatory)][string]$VsInstallPath)
    if (Test-EwasteVisualStudioEnvironment $VsInstallPath) { return }
    $module = Join-Path $VsInstallPath 'Common7/Tools/Microsoft.VisualStudio.DevShell.dll'
    if (!(Test-Path -LiteralPath $module -PathType Leaf)) {
        throw "Visual Studio Build Tools developer shell is missing from '$VsInstallPath'. Cargo was not started."
    }
    try {
        Import-Module $module -ErrorAction Stop
        Enter-VsDevShell -VsInstallPath $VsInstallPath -SkipAutomaticLocation -DevCmdArguments '-arch=x64 -host_arch=x64' -ErrorAction Stop
    } catch {
        throw "Could not initialize the Visual Studio x64 toolchain. Cargo was not started. $($_.Exception.Message)"
    }
    if (!(Test-EwasteVisualStudioEnvironment $VsInstallPath)) {
        throw "Visual Studio setup did not provide cl.exe/link.exe from '$VsInstallPath' with x64 host/target and matching headers/libraries. Cargo was not started."
    }
}
