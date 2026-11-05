# Mouser shell integration for PowerShell (pwsh 7+, Windows PowerShell 5.1).
# Load it from $PROFILE:   mouser shell-integration pwsh | Out-String | Invoke-Expression
# It reports the folder (OSC 7) and, at each prompt, moves to the folder
# commander's sync (ctrl+alt+s) left in this session's sync file: a builtin
# Set-Location in the prompt function, so nothing is typed and nothing reaches
# the history. PowerShell only runs its prompt after a command, so the change
# lands at the next prompt (press Enter on an empty line to take it at once).
if ($global:__mouser_si) { return }
$global:__mouser_si = $true
$global:__mouser_sync = Join-Path ([System.IO.Path]::GetTempPath()) "mouser-sync-$PID"
$global:__mouser_prompt = $function:prompt

function global:__mouser_pct([string]$s) {
  $o = New-Object System.Text.StringBuilder
  foreach ($b in [System.Text.Encoding]::UTF8.GetBytes($s)) {
    $c = [char]$b
    if ($b -lt 128 -and $c -match '[A-Za-z0-9._~/-]') { [void]$o.Append($c) } else { [void]$o.AppendFormat('%{0:X2}', $b) }
  }
  $o.ToString()
}

function global:prompt {
  if (Test-Path -LiteralPath $global:__mouser_sync) {
    $d = Get-Content -LiteralPath $global:__mouser_sync -Raw
    Remove-Item -LiteralPath $global:__mouser_sync -ErrorAction SilentlyContinue
    if ($d) { Set-Location -LiteralPath $d.TrimEnd("`r", "`n") -ErrorAction SilentlyContinue }
  }
  $e = [char]27
  $p = (Get-Location).ProviderPath -replace '\\', '/'
  if (-not $p.StartsWith('/')) { $p = '/' + $p }
  $h = [System.Net.Dns]::GetHostName()
  $f = __mouser_pct $global:__mouser_sync
  "$e]7;file://$h$(__mouser_pct $p)$e\${e}P=Fzsa;v=0,shell=pwsh,caps=cwdfile,file=$f$e\" + (& $global:__mouser_prompt)
}
