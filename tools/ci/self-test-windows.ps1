# Runs one app self-test on Windows and checks its result (CI and release builds, under WARP):
#
#   pwsh tools/ci/self-test-windows.ps1 -Exe target\release\device-studio.exe -Name windows-part1 -Part 1
#
# Without -Part the whole self-test runs. The run's stdout and stderr go to selftest-<name>.txt
# and selftest-<name>-stderr.txt (CI uploads them). It passes when the app printed
# "self-test: P / N checks passed" with P = N (skipped checks are not counted in N) and exited
# with status 0. FAIL and SKIP lines are echoed either way, and the tail of stderr when there is
# no summary. Set WGPU_BACKEND=dx12 and SLINT_WGPU_CPU=1 first: the runners have no GPU.
param(
    [Parameter(Mandatory = $true)][string]$Exe,
    [Parameter(Mandatory = $true)][string]$Name,
    [int]$Part = 0
)
$ErrorActionPreference = 'Stop'
$out = "selftest-$Name.txt"
$err = "selftest-$Name-stderr.txt"
$argv = @('--self-test', '--no-audio')
if ($Part -gt 0) { $argv += @('--self-test-part', "$Part") }
$start = Get-Date
$p = Start-Process -FilePath $Exe -ArgumentList $argv -NoNewWindow -Wait -PassThru -RedirectStandardOutput $out -RedirectStandardError $err
"self-test ($Name): exit code 0x{0:X8} after {1:N0} s" -f $p.ExitCode, ((Get-Date) - $start).TotalSeconds
Select-String -Path $out -Pattern '^(FAIL|SKIP)' | ForEach-Object { $_.Line }
Select-String -Path $err -Pattern 'frames waited for the GPU|^self-test: part ' | ForEach-Object { $_.Line }
$line = (Select-String -Path $out -Pattern '^self-test: (\d+) / (\d+) checks passed').Matches | Select-Object -First 1
if (-not $line) { Get-Content $err -Tail 30; throw "no self-test summary ($Name)" }
$line.Value
if ($line.Groups[1].Value -ne $line.Groups[2].Value) { throw "self-test failures ($Name)" }
if ($p.ExitCode -ne 0) { throw "the self-test passed but the app exited with 0x{0:X8} ($Name)" -f $p.ExitCode }
