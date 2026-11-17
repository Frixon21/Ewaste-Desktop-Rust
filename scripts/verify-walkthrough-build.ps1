$ErrorActionPreference='Stop'
$appRoot=Split-Path $PSScriptRoot -Parent
$inputFile=Join-Path $appRoot 'target/release/ewaste-counter.d'
$releaseFile=Join-Path $appRoot 'target/release/ewaste-counter.exe'
$inputs=Get-Content -LiteralPath $inputFile -Raw

function Split-CargoDependencyWords {
    param([string]$Text)
    $words=[Collections.Generic.List[string]]::new()
    $word=[Text.StringBuilder]::new()
    for($index=0;$index -lt $Text.Length;$index++) {
        $character=$Text[$index]
        # Cargo escapes spaces in make paths but leaves Windows separators intact.
        if($character -eq '\' -and $index+1 -lt $Text.Length -and [char]::IsWhiteSpace($Text[$index+1])) {
            [void]$word.Append($Text[$index+1]);$index++
        } elseif([char]::IsWhiteSpace($character)) {
            if($word.Length){$words.Add($word.ToString());[void]$word.Clear()}
        } else {[void]$word.Append($character)}
    }
    if($word.Length){$words.Add($word.ToString())}
    return $words.ToArray()
}

function ConvertTo-CargoDependencyPath {
    param([string]$Value)
    if([string]::IsNullOrWhiteSpace($Value)){throw 'Dependency information contains an empty path.'}
    if(![IO.Path]::IsPathRooted($Value)){$Value=Join-Path $appRoot $Value}
    # Do not require existence: rustc #[path] entries may retain '..' segments.
    return [IO.Path]::GetFullPath($Value).Replace('\','/').TrimEnd('/')
}

$required=@(
    'src/counter/walkthrough_ui.rs',
    'src/counter/walkthrough_details.rs',
    'src/counter/walkthrough_price_snapshot.rs',
    'src/counter/printer/printer_windows.rs',
    'src/counter/printer/printer_ui.rs',
    'src/counter/scanner/scanner.rs',
    'src/counter/scanner/scanner_aamva.rs',
    'src/counter/scanner/scanner_ui.rs',
    'src/store/mod.rs',
    'src/store/compatibility.rs',
    'src/store/permissions.rs',
    'src/store/records.rs',
    'src/store/receipt_delivery.rs',
    'src/shared/model/operator_snapshot.rs',
    'src/shared/receipts/receipt_dispatch.rs',
    'src/shared/receipts/receipt_dispatch_binding.rs',
    'src/shared/receipts/receipt_dispatch_recovery.rs',
    'src/shared/ui/register_common.rs'
)
$excludedDirectories=@('src/extended/','src/tests/')
# Retain exact old-flat exclusions so a mixed stale build cannot hide old modules.
$excluded=@('ui.rs','ui_auth.rs','ui_intake.rs','intake_ui.rs','intake_review.rs','intake_export.rs','inventory.rs','inventory_types.rs','inventory_replay.rs','inventory_export.rs','inventory_ui.rs','inventory_ui_forms.rs','ui_inventory.rs','store_inventory.rs','adjustments.rs','adjustment_ui.rs','ui_adjustments.rs','store_adjustments.rs','price_overrides.rs','price_override_ui.rs','ui_price_overrides.rs','store_price_overrides.rs','closeout.rs','closeout_ui.rs','catalog_ui.rs','legacy_exchange.rs','legacy_exchange_file.rs','walkthrough_auth.rs','auth.rs','auth_ui.rs','store_auth.rs') | ForEach-Object {'src/'+$_}

$releasePath=ConvertTo-CargoDependencyPath $releaseFile
$dependencies=[Collections.Generic.HashSet[string]]::new([StringComparer]::OrdinalIgnoreCase)
$matchedReleaseRule=$false
$logicalInputs=[regex]::Replace($inputs,'\\\r?\n',' ')
foreach($line in ($logicalInputs -split '\r?\n')) {
    # The rule colon is followed by whitespace/end; Windows drive colons are not.
    $separator=[regex]::Match($line,'(?<!\\):(?=\s|$)')
    if(!$separator.Success){continue}
    $targets=@(Split-CargoDependencyWords $line.Substring(0,$separator.Index))
    $isRelease=$false
    foreach($target in $targets){if((ConvertTo-CargoDependencyPath $target) -eq $releasePath){$isRelease=$true}}
    if(!$isRelease){continue}
    $matchedReleaseRule=$true
    foreach($dependency in @(Split-CargoDependencyWords $line.Substring($separator.Index+1))) {
        [void]$dependencies.Add((ConvertTo-CargoDependencyPath $dependency))
    }
}
if(!$matchedReleaseRule){throw 'Dependency information has no rule for this release executable.'}

foreach($directory in $excludedDirectories) {
    $excludedPath=ConvertTo-CargoDependencyPath $directory
    $prefix=$excludedPath+'/'
    foreach($dependency in $dependencies) {
        if($dependency -eq $excludedPath -or $dependency.StartsWith($prefix,[StringComparison]::OrdinalIgnoreCase)) {
            throw "Normal release compiled an excluded workflow directory: $dependency"
        }
    }
}
foreach($name in $excluded) {
    if($dependencies.Contains((ConvertTo-CargoDependencyPath $name))){throw "Normal release compiled an excluded workflow module: $name"}
}
foreach($name in $required) {
    if(!$dependencies.Contains((ConvertTo-CargoDependencyPath $name))){throw "Walkthrough release is missing expected source: $name"}
}

$output=Join-Path $appRoot '.verification/walkthrough-build-inputs.txt'
$inputs | Set-Content -LiteralPath $output
[ordered]@{required_sources=$required;excluded_sources=$excluded;excluded_directories=$excludedDirectories;inputs_artifact=$output;release_sha256=(Get-FileHash -LiteralPath $releaseFile).Hash} | ConvertTo-Json -Depth 3 | Set-Content -LiteralPath (Join-Path $appRoot '.verification/walkthrough-build-verification.json')
'Verified normal release excludes the deferred workflow modules.'
