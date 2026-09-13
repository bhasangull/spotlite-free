param([int]$Samples = 10, [int]$IntervalSeconds = 3)
$ErrorActionPreference = 'Stop'
# No cookies, tokens, command lines or unrelated browser memory in output.
$measurements = for ($sample = 0; $sample -lt $Samples; $sample++) {
    $processes = @(Get-CimInstance Win32_Process)
    $owned = [System.Collections.Generic.HashSet[int]]::new()
    foreach ($process in $processes) {
        if (($process.Name -eq 'msedge.exe' -and ($process.CommandLine -like '*SpotLiteFree\PlaybackBrowser*' -or $process.CommandLine -like '*SpotLiteFree\IPv6Browser*')) -or
            ($process.Name -like 'spotlite-free*.exe' -and $process.ExecutablePath -like '*c-users-thea-downloads-librespot-dev*')) {
            [void]$owned.Add([int]$process.ProcessId)
        }
    }
    do {
        $added = $false
        foreach ($process in $processes) {
            if ($owned.Contains([int]$process.ParentProcessId) -and $process.Name -eq 'msedge.exe') {
                if ($owned.Add([int]$process.ProcessId)) { $added = $true }
            }
        }
    } while ($added)
    $selected = @($processes | Where-Object { $owned.Contains([int]$_.ProcessId) })
    $privateResident = @(Get-CimInstance Win32_PerfFormattedData_PerfProc_Process | Where-Object { $owned.Contains([int]$_.IDProcess) })
    $accounts = @()
    try {
        $accounts = @((Invoke-RestMethod -Uri 'http://127.0.0.1:43116/accounts' -TimeoutSec 5) | ForEach-Object {
            [pscustomobject]@{ id=$_.instance; state=$_.runtime.state; position=$_.runtime.position; duration=$_.runtime.duration }
        })
    } catch {}
    [pscustomobject]@{
        timestamp = [DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds()
        processCount = $selected.Count
        # Sum includes shared resident pages; private commit is reported separately.
        workingSetMiB = [Math]::Round(($selected | Measure-Object WorkingSetSize -Sum).Sum / 1MB, 2)
        privateCommitMiB = [Math]::Round(($selected | Measure-Object PrivatePageCount -Sum).Sum / 1MB, 2)
        privateWorkingSetMiB = [Math]::Round(($privateResident | Measure-Object WorkingSetPrivate -Sum).Sum / 1MB, 2)
        accounts = $accounts
    }
    if ($sample -lt $Samples - 1) { Start-Sleep -Seconds $IntervalSeconds }
}
$measurements | ConvertTo-Json -Depth 5
