<#
.SYNOPSIS
  Check the compiled MSI's install scope, publisher, shortcuts, files and file registrations, without installing it.
.EXAMPLE
  pwsh packaging/windows/test-msi.ps1 dist/release/xmlcraft-0.1.0-windows-x64.msi
#>
param([Parameter(Mandatory)] [string] $Path)
$ErrorActionPreference = 'Stop'
$Installer = New-Object -ComObject WindowsInstaller.Installer
$Database = $Installer.OpenDatabase((Resolve-Path -LiteralPath $Path).Path, 0)

function Read-Rows([string] $Sql, [int] $Columns) {
  $view = $Database.OpenView($Sql)
  try {
    [void] $view.Execute()
    $rows = @()
    while ($record = $view.Fetch()) { $rows += , @(for ($i = 1; $i -le $Columns; $i++) { $record.StringData($i) }) }
    return , $rows
  } finally { [void] $view.Close() }
}

function Read-Row([string] $Sql, [int] $Columns) {
  $rows = Read-Rows $Sql $Columns
  if ($rows.Count -eq 0) { throw "MSI row missing: $Sql" }
  return , $rows[0]
}

function Assert-Equal($Actual, $Expected, [string] $What) {
  if ($Actual -cne $Expected) { throw "$What`: expected '$Expected', got '$Actual'" }
}

# The UpgradeCode ties every release to the same product; changing it would make updates install beside the old version.
$upgradeCode = Read-Row 'SELECT `Value` FROM `Property` WHERE `Property` = ''UpgradeCode''' 1
Assert-Equal $upgradeCode[0] '{5CE729A2-80AA-4E57-8DC3-6B3014812640}' 'MSI UpgradeCode'
$manufacturer = Read-Row 'SELECT `Value` FROM `Property` WHERE `Property` = ''Manufacturer''' 1
Assert-Equal $manufacturer[0] 'The XmlCraft contributors' 'MSI manufacturer'
$product = Read-Row 'SELECT `Value` FROM `Property` WHERE `Property` = ''ProductName''' 1
Assert-Equal $product[0] 'XmlCraft' 'MSI product name'
$allUsers = Read-Row 'SELECT `Value` FROM `Property` WHERE `Property` = ''ALLUSERS''' 1
Assert-Equal $allUsers[0] '1' 'MSI install scope (per machine)'

# Per-user overrides are refused before costing, as in a per-machine package.
$scopeMessage = 'XmlCraft must be installed for all users. Run setup with administrator privileges and ALLUSERS=1; per-user installation is not supported.'
$scopeCondition = Read-Row ('SELECT `Condition` FROM `LaunchCondition` WHERE `Description` = ''' + $scopeMessage + '''') 1
Assert-Equal $scopeCondition[0] 'ALLUSERS = 1 OR (Installed AND REMOVE = "ALL")' 'launch condition'

# The program and the CLI are installed.
foreach ($file in @('xmlcraft.exe', 'xmlcraft-cli.exe', 'LICENSE-MIT', 'LICENSE-APACHE', 'NOTICE')) {
  $row = Read-Row ('SELECT `FileName` FROM `File` WHERE `FileName` LIKE ''%' + $file + '''') 1
  if (-not $row[0]) { throw "the MSI does not install $file" }
}

# Plain (non-advertised) shortcuts to xmlcraft.exe, each in its own component; the desktop one is gated by INSTALLDESKTOPSHORTCUT.
foreach ($entry in @(@('StartMenuShortcut', 'ProgramMenuFolder', 'XmlcraftStartMenuShortcut', ''),
                     @('DesktopShortcut', 'DesktopFolder', 'XmlcraftDesktopShortcut', 'INSTALLDESKTOPSHORTCUT = 1'))) {
  $row = Read-Row ('SELECT `Directory_`, `Target`, `Icon_`, `WkDir`, `Component_` FROM `Shortcut` WHERE `Shortcut` = ''' + $entry[0] + '''') 5
  Assert-Equal $row[0] $entry[1] "$($entry[0]) directory"
  Assert-Equal $row[1] '[#XmlcraftExe]' "$($entry[0]) target"
  Assert-Equal $row[2] 'XmlcraftIcon.ico' "$($entry[0]) icon"
  Assert-Equal $row[3] 'INSTALLFOLDER' "$($entry[0]) working directory"
  Assert-Equal $row[4] $entry[2] "$($entry[0]) component"
  $component = Read-Row ('SELECT `Directory_`, `Condition` FROM `Component` WHERE `Component` = ''' + $entry[2] + '''') 2
  Assert-Equal $component[0] $entry[1] "$($entry[2]) directory"
  Assert-Equal $component[1] $entry[3] "$($entry[2]) condition"
}

# XmlCraft is offered for XML-family files but never made the default: each extension gets an OpenWithProgids value and no value
# of its own.
$registry = Read-Rows 'SELECT `Key`, `Name`, `Value` FROM `Registry`' 3
foreach ($ext in @('.xml', '.xsd', '.dtd', '.ent', '.mod', '.xsl', '.xslt', '.wsdl')) {
  $open = @($registry | Where-Object { $_[0] -eq "Software\Classes\$ext\OpenWithProgids" -and $_[1] -eq 'XmlCraft.Document' })
  if ($open.Count -ne 1) { throw "$ext is not offered under Open with" }
  $takeover = @($registry | Where-Object { $_[0] -eq "Software\Classes\$ext" -and -not $_[1] })
  if ($takeover.Count -ne 0) { throw "$ext default value is set: XmlCraft must not become the default" }
  $cap = @($registry | Where-Object { $_[0] -eq 'Software\XmlCraft\Capabilities\FileAssociations' -and $_[1] -eq $ext })
  if ($cap.Count -ne 1) { throw "$ext is missing from the Capabilities" }
}
$apps = @($registry | Where-Object { $_[0] -eq 'Software\RegisteredApplications' -and $_[1] -eq 'XmlCraft' })
if ($apps.Count -ne 1) { throw 'XmlCraft is not registered under RegisteredApplications' }
Write-Output "ok $(Split-Path -Leaf $Path)"
