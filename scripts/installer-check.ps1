# Installs an Inkwell MSI, checks what it registered and that "Open with Inkwell" forwards to one running
# instance, uninstalls, and checks nothing is left. Needs an elevated shell for the per-machine MSI.
#   pwsh -File scripts/installer-check.ps1 -Msi <path to Inkwell_0.1.0_x64_en-US.msi> -Pdf a.pdf,b.pdf [-Desktop]
# Works on copies you give it (-Pdf): never point it at real documents. It does not change the default PDF app
# and uses a scratch INKWELL_DATA_DIR, so the user's recent list is not touched.
# -User checks a per-user build (HKCU roots, the user's Start menu) instead of the per-machine one.
param(
  [Parameter(Mandatory)][string]$Msi,
  [string[]]$Pdf = @(),
  [switch]$Desktop,
  [switch]$User
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
function UserChoice { (Get-ItemProperty 'HKCU:\SOFTWARE\Microsoft\Windows\CurrentVersion\Explorer\FileExts\.pdf\UserChoice' -ErrorAction SilentlyContinue).ProgId }
function PdfDefault { (Get-Item "$classes\.pdf" -ErrorAction SilentlyContinue).GetValue('') }

if (-not $User) {
  $admin = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
  if (-not $admin) { throw 'The per-machine MSI needs an elevated PowerShell (or use -User with a per-user build).' }
}

$before = [pscustomobject]@{
  Choice = UserChoice; PdfDefault = PdfDefault
  SfaPdf = Test-Path "$classes\SystemFileAssociations\.pdf"
  SfaShell = Test-Path "$classes\SystemFileAssociations\.pdf\shell"
  PdfShell = Test-Path "$classes\.pdf\shell"
  OpenWith = (Get-Item "$classes\.pdf\OpenWithProgids" -ErrorAction SilentlyContinue).GetValueNames() -join ','
}
"Before: default-app choice '$($before.Choice)', .pdf default '$($before.PdfDefault)'"

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
Check ((UserChoice) -eq $before.Choice) "default PDF app untouched (UserChoice '$(UserChoice)')"
Check ((PdfDefault) -eq $before.PdfDefault) ".pdf default value untouched ('$(PdfDefault)')"

if ($Pdf.Count) {
  $env:INKWELL_DATA_DIR = Join-Path $log 'data'
  $env:INKWELL_SINGLE_INSTANCE = '1'
  $files = $Pdf | ForEach-Object { (Resolve-Path $_).Path }
  Start-Process -FilePath $files[0] -Verb 'Inkwell.Open'
  Start-Sleep 6
  $files | Select-Object -Skip 1 | ForEach-Object { Start-Process -FilePath $_ -Verb 'Inkwell.Open' }
  Start-Sleep 4
  $procs = @(Get-Process inkwell -ErrorAction SilentlyContinue)
  Check ($procs.Count -eq 1) "one running instance for $($files.Count) launches ($($procs.Count) process)"
  # the window title is the active document: the last forwarded file opened last
  $title = ($procs | Select-Object -First 1).MainWindowTitle
  $name = [IO.Path]::GetFileNameWithoutExtension($files[-1])
  Check ($title -like "*$name*") "window shows the last file ('$title')"
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
Check (-not (Has "$hive\SOFTWARE\Inkwell")) 'HKLM/HKCU Software\Inkwell gone'
Check (-not (Has "$classes\Inkwell.PDF")) 'ProgId gone'
Check (-not (Has "$classes\.pdf\OpenWithProgids" 'Inkwell.PDF')) 'Open with entry gone'
Check (-not (Has "$hive\SOFTWARE\RegisteredApplications" 'Inkwell')) 'registered application gone'
Check (-not (Has "$classes\Applications\inkwell.exe")) 'Applications entry gone'
Check (-not (Has "$classes\SystemFileAssociations\.pdf\shell\Inkwell.Open")) 'verb gone'
Check ((Test-Path "$classes\SystemFileAssociations\.pdf") -eq $before.SfaPdf) "SystemFileAssociations\.pdf back to how it was ($($before.SfaPdf))"
Check ((Test-Path "$classes\SystemFileAssociations\.pdf\shell") -eq $before.SfaShell) "SystemFileAssociations\.pdf\shell back to how it was ($($before.SfaShell))"
Check ((Test-Path "$classes\.pdf\shell") -eq $before.PdfShell) ".pdf\shell back to how it was ($($before.PdfShell))"
Check (((Get-Item "$classes\.pdf\OpenWithProgids" -ErrorAction SilentlyContinue).GetValueNames() -join ',') -eq $before.OpenWith) 'Open with list back to how it was'
Check ((UserChoice) -eq $before.Choice -and (PdfDefault) -eq $before.PdfDefault) 'default PDF app still untouched'
"The user data folder is left in place by design: $env:APPDATA\app.inkwell.desktop"
if ($fail) { "$fail check(s) FAILED"; exit 1 } else { 'all checks passed' }
