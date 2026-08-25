$tempDir = Join-Path $env:TEMP 'fdm-benchmark'
New-Item -ItemType Directory -Force -Path $tempDir | Out-Null

Write-Host '=== Benchmark 1: Standard 1 Connection (Sequential) ==='
$sw1 = [System.Diagnostics.Stopwatch]::StartNew()
& target\release\fdm.exe get --sequential --flat --out $tempDir --name 'test_seq.bin' https://ash-speed.hetzner.com/100MB.bin
$sw1.Stop()
$seqSec = [math]::Round($sw1.Elapsed.TotalSeconds, 2)
$seqSpeed = [math]::Round(100.0 / $seqSec, 2)
Write-Host "1 Stream Time: $($seqSec)s | Speed: $($seqSpeed) MB/s"

Write-Host ''
Write-Host '=== Benchmark 2: 16 Parallel Connections ==='
$sw2 = [System.Diagnostics.Stopwatch]::StartNew()
& target\release\fdm.exe get -n 16 --flat --out $tempDir --name 'test_16.bin' https://ash-speed.hetzner.com/100MB.bin
$sw2.Stop()
$p16Sec = [math]::Round($sw2.Elapsed.TotalSeconds, 2)
$p16Speed = [math]::Round(100.0 / $p16Sec, 2)
Write-Host "16 Streams Time: $($p16Sec)s | Speed: $($p16Speed) MB/s"

Write-Host ''
Write-Host '=== Benchmark 3: 64 Parallel Connections (Tuned Mode) ==='
$sw3 = [System.Diagnostics.Stopwatch]::StartNew()
& target\release\fdm.exe get -n 64 --flat --out $tempDir --name 'test_64.bin' https://ash-speed.hetzner.com/100MB.bin
$sw3.Stop()
$p64Sec = [math]::Round($sw3.Elapsed.TotalSeconds, 2)
$p64Speed = [math]::Round(100.0 / $p64Sec, 2)
Write-Host "64 Streams Time: $($p64Sec)s | Speed: $($p64Speed) MB/s"

Remove-Item -Recurse -Force $tempDir
