<#
.SYNOPSIS
Point-and-click installer for Deep Server Analyzer. Opened by DSA-Installer.cmd, which elevates first.

.DESCRIPTION
Install runs Install-DSA.ps1 (kit checks, temporary certificate trust, expiry task, silent setup).
The up arrow adds an hour to the trust window. The red X opens a PowerShell window that removes the
trust and prints a check of each store and the task. Closing this window never stops the timer: the
DSA-TrustExpiry task removes trust at expiry.

.PARAMETER SelfTest
Build the window and check its controls without showing it.
#>
[CmdletBinding()]
param(
    [switch]$SelfTest
)

$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName PresentationFramework
Import-Module (Join-Path $PSScriptRoot 'DsaTrust.psm1') -Force

$installScript = Join-Path $PSScriptRoot 'Install-DSA.ps1'
$removeScript = Join-Path $PSScriptRoot 'Remove-Trust.ps1'
$dataRemoveScript = Join-Path (Get-DsaDataDir) 'Remove-Trust.ps1'
$arrowUp = [string][char]0x25B2
$cross = [string][char]0x2715

$xaml = @'
<Window xmlns="http://schemas.microsoft.com/winfx/2006/xaml/presentation"
        xmlns:x="http://schemas.microsoft.com/winfx/2006/xaml"
        Title="Deep Server Analyzer installer" Width="420" Height="330"
        WindowStartupLocation="CenterScreen" ResizeMode="NoResize" Background="#1B1D22"
        FontFamily="Segoe UI" Foreground="#E8EAED">
  <Grid Margin="24">
    <Grid.RowDefinitions>
      <RowDefinition Height="Auto"/>
      <RowDefinition Height="*"/>
      <RowDefinition Height="Auto"/>
    </Grid.RowDefinitions>
    <TextBlock Grid.Row="0" Text="Deep Server Analyzer" FontSize="20" FontWeight="SemiBold"/>

    <StackPanel Grid.Row="1" x:Name="PanelIdle" Margin="0,16,0,0">
      <TextBlock x:Name="IdleText" TextWrapping="Wrap" Foreground="#A8ADB5"/>
      <Button x:Name="InstallButton" Content="Install" Height="44" Margin="0,24,0,0" FontSize="16"
              Background="#2F6FEB" Foreground="White" BorderThickness="0"/>
    </StackPanel>

    <StackPanel Grid.Row="1" x:Name="PanelBusy" Margin="0,16,0,0" Visibility="Collapsed">
      <TextBlock Text="Installing..." FontSize="16"/>
      <ProgressBar IsIndeterminate="True" Height="6" Margin="0,16,0,12"/>
      <TextBlock x:Name="BusyText" TextWrapping="Wrap" Foreground="#A8ADB5"/>
    </StackPanel>

    <StackPanel Grid.Row="1" x:Name="PanelActive" Margin="0,16,0,0" Visibility="Collapsed">
      <TextBlock Text="Installed. The certificate is trusted for" Foreground="#A8ADB5"/>
      <TextBlock x:Name="Clock" FontSize="44" FontWeight="Light" Margin="0,4,0,0"/>
      <TextBlock x:Name="ExpiresText" Foreground="#A8ADB5"/>
      <Grid Margin="0,20,0,0">
        <Grid.ColumnDefinitions>
          <ColumnDefinition Width="*"/>
          <ColumnDefinition Width="12"/>
          <ColumnDefinition Width="*"/>
        </Grid.ColumnDefinitions>
        <Button Grid.Column="0" x:Name="ExtendButton" Height="44" FontSize="16"
                Background="#2F6FEB" Foreground="White" BorderThickness="0"/>
        <Button Grid.Column="2" x:Name="KillButton" Height="44" FontSize="16"
                Background="#D93636" Foreground="White" BorderThickness="0"/>
      </Grid>
    </StackPanel>

    <StackPanel Grid.Row="1" x:Name="PanelDone" Margin="0,16,0,0" Visibility="Collapsed">
      <TextBlock x:Name="DoneTitle" FontSize="18" FontWeight="SemiBold" TextWrapping="Wrap"/>
      <TextBlock x:Name="DoneText" TextWrapping="Wrap" Foreground="#A8ADB5" Margin="0,8,0,0"/>
      <Button x:Name="CloseButton" Content="Close" Height="44" Margin="0,24,0,0" FontSize="16"
              Background="#2B2E35" Foreground="White" BorderThickness="0"/>
    </StackPanel>

    <TextBlock Grid.Row="2" x:Name="Footer" FontSize="11" Foreground="#6F7580" TextWrapping="Wrap"/>
  </Grid>
</Window>
'@

$window = [Windows.Markup.XamlReader]::Load((New-Object System.Xml.XmlNodeReader ([xml]$xaml)))
$names = 'PanelIdle', 'PanelBusy', 'PanelActive', 'PanelDone', 'IdleText', 'InstallButton', 'BusyText',
'Clock', 'ExpiresText', 'ExtendButton', 'KillButton', 'DoneTitle', 'DoneText', 'CloseButton', 'Footer'
$ui = @{}
foreach ($name in $names) {
    $ui[$name] = $window.FindName($name)
    if (-not $ui[$name]) { throw "Window is missing the control $name" }
}

$ui.ExtendButton.Content = "$arrowUp  +1 h"
$ui.KillButton.Content = "$cross  Remove trust"
$ui.ExtendButton.FontFamily = 'Segoe UI Symbol'
$ui.KillButton.FontFamily = 'Segoe UI Symbol'
$ui.Footer.Text = 'Closing this window keeps the timer running; the DSA-TrustExpiry task removes trust at expiry.'

if ($SelfTest) {
    Write-Host 'DSA-Installer window built; all controls found.'
    exit 0
}

$principal = New-Object Security.Principal.WindowsPrincipal([Security.Principal.WindowsIdentity]::GetCurrent())
if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    [void][Windows.MessageBox]::Show('Run this through DSA-Installer.cmd so it can ask for administrator rights.', 'Deep Server Analyzer', 'OK', 'Warning')
    exit 1
}

$script:proc = $null
$script:mode = 'idle'

function Show-Panel {
    param([string]$Mode)
    $script:mode = $Mode
    $map = @{ idle = 'PanelIdle'; busy = 'PanelBusy'; active = 'PanelActive'; done = 'PanelDone' }
    foreach ($key in $map.Keys) {
        $ui[$map[$key]].Visibility = if ($key -eq $Mode) { 'Visible' } else { 'Collapsed' }
    }
}

function Show-Done {
    param([string]$Title, [string]$Text, [string]$Color = '#E8EAED')
    $ui.DoneTitle.Text = $Title
    $ui.DoneTitle.Foreground = (New-Object Windows.Media.BrushConverter).ConvertFromString($Color)
    $ui.DoneText.Text = $Text
    Show-Panel 'done'
}

function Get-LastLogLine {
    $log = Join-Path (Get-DsaDataDir) 'trust.log'
    if (-not (Test-Path -LiteralPath $log)) { return '' }
    $line = Get-Content -LiteralPath $log -Tail 1
    return ($line -replace '^\S+\s+\[\w+\]\s*', '')
}

function Start-KitProcess {
    param([string]$Script, [string[]]$Arguments, [string]$Style)
    $argList = @('-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', ('"{0}"' -f $Script)) + $Arguments
    $p = Start-Process -FilePath 'powershell.exe' -ArgumentList $argList -WindowStyle $Style -PassThru
    $null = $p.Handle   # keeps the exit code readable after the process ends
    return $p
}

$setup = Get-ChildItem -LiteralPath $PSScriptRoot -Filter '*-setup.exe' -File | Select-Object -First 1
$cer = Join-Path $PSScriptRoot 'dsa-signing.cer'
if ($setup -and (Test-Path -LiteralPath $cer)) {
    $ui.IdleText.Text = "Trusts the Deep Server Analyzer signing certificate ($((Get-CerThumbprint -CerPath $cer).Substring(0, 12))...) for 8 hours, installs the app, then removes the trust. Use the arrow to add time and the red button to remove it early."
}
else {
    $ui.IdleText.Text = 'The setup or certificate is missing from this folder. Extract the whole kit zip and open DSA-Installer.cmd from there.'
    $ui.InstallButton.IsEnabled = $false
}

$ui.InstallButton.Add_Click({
        $ui.BusyText.Text = 'Checking the kit...'
        Show-Panel 'busy'
        $script:proc = Start-KitProcess $installScript @('-NoPrompt', '-NoCountdown') 'Hidden'
    })

$ui.ExtendButton.Add_Click({
        $expiry = Add-TrustHours -RemoveScriptPath $dataRemoveScript
        if (-not $expiry) { Show-Done 'No active trust' 'The trust was already removed.' }
    })

$ui.KillButton.Add_Click({
        $ui.ExtendButton.IsEnabled = $false
        $ui.KillButton.IsEnabled = $false
        $ui.ExpiresText.Text = 'Removing trust in the PowerShell window...'
        $script:proc = Start-KitProcess $removeScript @('-Verify') 'Normal'
    })

$ui.CloseButton.Add_Click({ $window.Close() })

$timer = New-Object Windows.Threading.DispatcherTimer
$timer.Interval = [TimeSpan]::FromMilliseconds(500)
$timer.Add_Tick({
        if ($script:mode -eq 'busy') {
            if ($script:proc.HasExited) {
                if ($script:proc.ExitCode -eq 0) { $script:proc = $null; Show-Panel 'active' }
                else {
                    $detail = Get-LastLogLine
                    $script:proc = $null
                    Show-Done 'Install failed' "$detail`n`nAny trust added was removed again. Details: $(Join-Path (Get-DsaDataDir) 'trust.log')" '#FF6B6B'
                }
            }
            else { $ui.BusyText.Text = Get-LastLogLine }
        }
        if ($script:mode -eq 'active') {
            if ($script:proc -and $script:proc.HasExited) {
                $code = $script:proc.ExitCode
                $script:proc = $null
                if ($code -eq 0) { Show-Done 'Trust removed and verified' 'The certificate is gone from both stores and the expiry task is unregistered.' '#4CD964' }
                else { Show-Done 'Removal could not be verified' 'Check the PowerShell window output or trust.log, then run Remove-Trust.cmd as administrator.' '#FF6B6B' }
                return
            }
            # The removal window deletes the record before it waits for a key; let it finish first.
            if ($script:proc) { return }
            $record = Read-TrustExpiry
            if (-not $record) { Show-Done 'Trust is not active' 'Nothing is trusted any more.'; return }
            $left = $record.ExpiresUtc - [DateTime]::UtcNow
            if (Test-TrustDue -ExpiresUtc $record.ExpiresUtc) {
                Remove-ActiveTrust
                Show-Done 'Trust expired and was removed' 'The certificate is no longer trusted.' '#4CD964'
                return
            }
            $ui.Clock.Text = Format-Remaining $left
            if (-not $script:proc) {
                $ui.ExpiresText.Text = 'Expires at ' + $record.ExpiresUtc.ToLocalTime().ToString('HH:mm')
                $ui.ExtendButton.IsEnabled = $true
                $ui.KillButton.IsEnabled = $true
            }
        }
    })

# Reopening the installer while trust is active goes straight to the countdown.
$existing = Read-TrustExpiry
if ($existing -and -not (Test-TrustDue -ExpiresUtc $existing.ExpiresUtc)) { Show-Panel 'active' } else { Show-Panel 'idle' }
$timer.Start()
[void]$window.ShowDialog()
