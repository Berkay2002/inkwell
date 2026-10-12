# Installs an Inkwell MSI, checks what it registered and that "Open with Inkwell" forwards to one running
# instance, uninstalls, and checks nothing is left. Needs an elevated shell for the per-machine MSI.
#   pwsh -File scripts/installer-check.ps1 -Msi <path to Inkwell_0.1.0_x64_en-US.msi> -Pdf 'a.pdf;b.pdf' [-Desktop] [-Burst]
# Works on copies you give it (-Pdf): never point it at real documents. It does not change the default PDF app
# and uses a scratch INKWELL_DATA_DIR, so the user's recent list is not touched.
# -User checks a per-user build (HKCU roots, the user's Start menu) instead of the per-machine one.
param(
  [Parameter(Mandatory)][string]$Msi,
  [string]$Pdf = '', # copies of PDFs, separated by ;
  [switch]$Desktop,
  [switch]$User,
  [switch]$Burst # start every file at once instead of the first one, then the rest
)
$ErrorActionPreference = 'Stop'
$Msi = (Resolve-Path $Msi).Path
$log = Join-Path $env:TEMP 'inkwell-installer-check'
New-Item -ItemType Directory -Force $log | Out-Null
$hive = if ($User) { 'HKCU:' } else { 'HKLM:' }
$classes = "$hive\SOFTWARE\Classes"
$startMenu = [Environment]::GetFolderPath($(if ($User) { 'Programs' } else { 'CommonPrograms' }))
$desktopDir = [Environment]::GetFolderPath($(if ($User) { 'DesktopDirectory' } else { 'CommonDesktopDirectory' }))
$fail = 0
function Check($ok, $what) {
  if ($ok) { "  ok    $what" } else { "  FAIL  $what"; $script:fail++ }
}
function Has($path, $name = $null) {
  if (-not (Test-Path -LiteralPath $path)) { return $false }
  if ($null -eq $name) { return $true }
  $null -ne (Get-Item -LiteralPath $path).GetValue($name, $null)
}
function ValueNames($path) {
  $k = Get-Item -LiteralPath $path -ErrorAction SilentlyContinue
  if ($k) { ($k.GetValueNames() | Sort-Object) -join ',' } else { '' }
}
# Every value under the key where Windows keeps the user's PDF choice (UserChoice, UserChoiceLatest, Open with lists).
function Choice {
  $root = 'HKCU:\SOFTWARE\Microsoft\Windows\CurrentVersion\Explorer\FileExts\.pdf'
  $keys = @(Get-Item $root -ErrorAction SilentlyContinue) + @(Get-ChildItem $root -Recurse -ErrorAction SilentlyContinue)
  ($keys | ForEach-Object { $k = $_; $k.GetValueNames() | ForEach-Object { "$($k.Name)|$_|$($k.GetValue($_))" } }) -join ';'
}
function PdfDefault { (Get-Item "$classes\.pdf" -ErrorAction SilentlyContinue).GetValue('') }

if (-not $User) {
  $admin = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
  if (-not $admin) { throw 'The per-machine MSI needs an elevated PowerShell (or use -User with a per-user build).' }
}

$before = [pscustomobject]@{
  Choice = Choice; PdfDefault = PdfDefault
  SfaPdf = Test-Path "$classes\SystemFileAssociations\.pdf"
  SfaShell = Test-Path "$classes\SystemFileAssociations\.pdf\shell"
  PdfShell = Test-Path "$classes\.pdf\shell"
  OpenWith = ValueNames "$classes\.pdf\OpenWithProgids"
  OpenWithKey = Test-Path "$classes\.pdf\OpenWithProgids"
}
"Before: .pdf default '$($before.PdfDefault)', $(@($before.Choice -split ';' | Where-Object { $_ }).Count) values under FileExts\.pdf"

$msiArgs = @('/i', "`"$Msi`"", '/qn', '/norestart', '/l*v', "`"$log\install.log`"")
if ($Desktop) { $msiArgs += 'DESKTOPSHORTCUT=1' }
$p = Start-Process msiexec -ArgumentList $msiArgs -Wait -PassThru
"Install: msiexec exit $($p.ExitCode) (log $log\install.log)"
Check ($p.ExitCode -eq 0) 'install succeeded'

$dir = (Get-ItemProperty "$hive\SOFTWARE\Inkwell" -ErrorAction SilentlyContinue).InstallDir
Check ($dir -and (Test-Path "$dir\inkwell.exe")) "app installed ($dir)"
Check (Test-Path "$dir\pdf-document.ico") 'PDF document icon installed'
Check (Test-Path "$dir\licenses\THIRD-PARTY-NOTICES.txt") 'licence notices installed'
Check (Test-Path "$startMenu\Inkwell.lnk") 'Start-menu shortcut'
Check ((Test-Path "$desktopDir\Inkwell.lnk") -eq [bool]$Desktop) $(if ($Desktop) { 'desktop shortcut (asked for)' } else { 'no desktop shortcut (not asked for)' })
Check (Has "$classes\Inkwell.PDF") 'ProgId Inkwell.PDF'
Check (Has "$classes\Inkwell.PDF\DefaultIcon") 'PDF document icon registered'
Check (Has "$classes\Inkwell.PDF\shell\open\command") 'open command'
Check (Has "$classes\.pdf\OpenWithProgids" 'Inkwell.PDF') 'listed under Open with'
Check (Has "$hive\SOFTWARE\RegisteredApplications" 'Inkwell') 'registered application (Default apps)'
Check (Has "$hive\SOFTWARE\Inkwell\Capabilities\FileAssociations" '.pdf') 'capability .pdf'
Check (Has "$classes\Applications\inkwell.exe\shell\open\command") 'Applications entry'
Check (Has "$classes\SystemFileAssociations\.pdf\shell\Inkwell.Open\command") 'context-menu verb "Open with Inkwell"'
Check ((Choice) -eq $before.Choice) 'default PDF app untouched (nothing changed under FileExts\.pdf)'
Check ((PdfDefault) -eq $before.PdfDefault) ".pdf default value untouched ('$(PdfDefault)')"

$pdfs = @($Pdf -split ';' | Where-Object { $_ })
if ($pdfs.Count) {
  # Scratch data dir: the user's recent list is not touched. Forwarding is off for scratch runs unless asked for.
  $env:INKWELL_DATA_DIR = Join-Path $log 'data'
  Remove-Item -Recurse -Force $env:INKWELL_DATA_DIR -ErrorAction SilentlyContinue
  $env:INKWELL_SINGLE_INSTANCE = '1'
  $files = $pdfs | ForEach-Object { (Resolve-Path $_).Path }
  # ours: the processes that did not exist before (a path can show in 8.3 form, so it is not matched)
  $known = @(Get-Process inkwell -ErrorAction SilentlyContinue).Id
  if ($Burst) {
    # like a multi-select in Explorer: every launch within a few milliseconds, before the first window has loaded
    $files | ForEach-Object { Start-Process -FilePath $_ -Verb 'Inkwell.Open' }
    Start-Sleep 8
  } else {
    Start-Process -FilePath $files[0] -Verb 'Inkwell.Open'
    Start-Sleep 6
    $files | Select-Object -Skip 1 | ForEach-Object { Start-Process -FilePath $_ -Verb 'Inkwell.Open' }
    Start-Sleep 4
  }
  $procs = @(Get-Process inkwell -ErrorAction SilentlyContinue | Where-Object { $known -notcontains $_.Id })
  Check ($procs.Count -eq 1) "one running instance for $($files.Count) launches ($($procs.Count) process)"
  # the window title is the active document: with a few files the last forwarded one, with a burst any of them
  $title = ($procs | Select-Object -First 1).MainWindowTitle
  $names = @($files | ForEach-Object { [IO.Path]::GetFileName($_) })
  if ($files.Count -le 3) {
    Check ($title -like "*$($names[-1] -replace '\.pdf$','')*") "window shows the last file ('$title')"
  } else {
    Check (@($names | Where-Object { $title -like "*$($_ -replace '\.pdf$','')*" }).Count -ge 1) "window shows one of the files ('$title')"
  }
  # every file was opened: the recent list in the scratch data folder names each one (written as they open)
  $recents = Join-Path $env:INKWELL_DATA_DIR 'recents.json'
  $deadline = (Get-Date).AddSeconds(60)
  do {
    Start-Sleep 2
    $json = if (Test-Path $recents) { Get-Content $recents -Raw } else { '' }
    $seen = @($names | Where-Object { $json.Contains(('\\' + $_ + '"')) }).Count
  } while ($seen -lt $names.Count -and (Get-Date) -lt $deadline)
  Check ($seen -eq $names.Count) "all $($names.Count) files opened ($seen in the recent list)"
  $procs | Stop-Process -Force
  Start-Sleep 1
  Remove-Item Env:\INKWELL_DATA_DIR, Env:\INKWELL_SINGLE_INSTANCE
}

$p = Start-Process msiexec -ArgumentList '/x', "`"$Msi`"", '/qn', '/norestart', '/l*v', "`"$log\uninstall.log`"" -Wait -PassThru
"Uninstall: msiexec exit $($p.ExitCode) (log $log\uninstall.log)"
Check ($p.ExitCode -eq 0) 'uninstall succeeded'
Check (-not (Test-Path "$dir")) 'install folder gone'
Check (-not (Test-Path "$startMenu\Inkwell.lnk")) 'Start-menu shortcut gone'
Check (-not (Test-Path "$desktopDir\Inkwell.lnk")) 'desktop shortcut gone'
Check (-not (Has "$hive\SOFTWARE\Inkwell")) 'Software\Inkwell gone'
Check (-not (Has 'HKCU:\SOFTWARE\Inkwell')) 'HKCU Software\Inkwell (shortcut markers) gone'
Check (-not (Has "$classes\Inkwell.PDF")) 'ProgId gone'
Check (-not (Has "$classes\.pdf\OpenWithProgids" 'Inkwell.PDF')) 'Open with entry gone'
Check (-not (Has "$hive\SOFTWARE\RegisteredApplications" 'Inkwell')) 'registered application gone'
Check (-not (Has "$classes\Applications\inkwell.exe")) 'Applications entry gone'
Check (-not (Has "$classes\SystemFileAssociations\.pdf\shell\Inkwell.Open")) 'verb gone'
Check ((Test-Path "$classes\SystemFileAssociations\.pdf") -eq $before.SfaPdf) "SystemFileAssociations\.pdf back to how it was ($($before.SfaPdf))"
Check ((Test-Path "$classes\SystemFileAssociations\.pdf\shell") -eq $before.SfaShell) "SystemFileAssociations\.pdf\shell back to how it was ($($before.SfaShell))"
Check ((Test-Path "$classes\.pdf\shell") -eq $before.PdfShell) ".pdf\shell back to how it was ($($before.PdfShell))"
Check ((ValueNames "$classes\.pdf\OpenWithProgids") -eq $before.OpenWith) 'Open with list back to how it was'
Check ((Test-Path "$classes\.pdf\OpenWithProgids") -eq $before.OpenWithKey) ".pdf\OpenWithProgids key back to how it was ($($before.OpenWithKey))"
Check ((Choice) -eq $before.Choice -and (PdfDefault) -eq $before.PdfDefault) 'default PDF app still untouched'
"The user data folder is left in place by design: $env:APPDATA\app.inkwell.desktop"
if ($fail) { "$fail check(s) FAILED"; exit 1 } else { 'all checks passed' }
