<#
.SYNOPSIS
  Captures de reference du TUI de Claude Code (jalon J1), par script.

.DESCRIPTION
  Pour chaque scenario de scenarios.json :
    1. recree un dossier jetable dans le bac a sable (.claude-work/j1/sandbox/<scenario>) ;
    2. ouvre SA PROPRE fenetre Windows Terminal (wt.exe -w new, 120x40, titre unique)
       qui lance claude.exe dans ce dossier ;
    3. deroule les etapes : attente fixe, attente d'un texte visible (lu par UI Automation
       sur la fenetre), attente du fichier de fin de tour ecrit par un hook Stop, touches
       envoyees (SendKeys) uniquement si la fenetre est au premier plan ;
    4. capture la fenetre seule (PrintWindow, sinon CopyFromScreen sur son rectangle)
       en PNG + le texte visible brut (.uia.txt) ;
    5. ferme la fenetre et termine les processus qu'elle a lances.

  Aucune config utilisateur n'est modifiee : le theme et le hook Stop passent par
  --settings <fichier du scenario>, la fenetre par les options de ligne de commande de wt.

.PARAMETER Only
  Numeros de captures a (re)faire, ex. -Only 05 ou -Only 05,06. Le scenario qui
  contient la capture est rejoue en entier, seules les captures demandees sont ecrites.

.PARAMETER Scenario
  Nom(s) de scenario a jouer (ex. ctrlc), au lieu de -Only.

.PARAMETER OutDir
  Dossier des PNG (defaut : <depot>/.claude-work/j1/captures).

.PARAMETER List
  Affiche les scenarios et captures, sans rien lancer.
#>
[CmdletBinding()]
param(
  [string[]]$Only,
  [string[]]$Scenario,
  [string]$OutDir,
  [string]$SandboxRoot,
  [string]$ScenarioFile = (Join-Path $PSScriptRoot 'scenarios.json'),
  [string]$Claude = (Join-Path $env:USERPROFILE '.local\bin\claude.exe'),
  [switch]$List
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version 3

$RepoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
if (-not $OutDir) { $OutDir = Join-Path $RepoRoot '.claude-work\j1\captures' }
if (-not $SandboxRoot) { $SandboxRoot = Join-Path $RepoRoot '.claude-work\j1\sandbox' }
$Config = Get-Content -Raw -Encoding utf8 $ScenarioFile | ConvertFrom-Json

if ($List) {
  foreach ($s in $Config.scenarios) {
    $caps = @($s.steps | Where-Object { $_.do -eq 'capture' } | ForEach-Object { $_.id + '-' + $_.name })
    '{0,-14} {1}' -f $s.name, ($caps -join ', ')
  }
  return
}

# --- Win32 -----------------------------------------------------------------
Add-Type -AssemblyName System.Drawing, System.Windows.Forms, UIAutomationClient, UIAutomationTypes
if (-not ('CapWin' -as [type])) {
  $refs = if ($PSVersionTable.PSEdition -eq 'Core') {
    @('System.Drawing.Common', 'System.Drawing.Primitives', 'System.Collections', 'System.Runtime.InteropServices', 'System.Runtime', 'System.Private.Windows.GdiPlus', 'System.Private.Windows.Core') |
      Where-Object { try { [void][Reflection.Assembly]::Load($_); $true } catch { $false } }
  } else { @('System.Drawing') }
  Add-Type -ReferencedAssemblies $refs -TypeDefinition @'
using System;
using System.Collections.Generic;
using System.Drawing;
using System.Runtime.InteropServices;
using System.Text;
public static class CapWin {
  public delegate bool EnumProc(IntPtr h, IntPtr l);
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr l);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowText(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassName(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr h, IntPtr hdc, uint flags);
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
  [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
  [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr h, int cmd);
  [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr h, uint msg, IntPtr w, IntPtr l);
  [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
  [DllImport("user32.dll")] public static extern void keybd_event(byte vk, byte scan, uint flags, UIntPtr extra);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  [DllImport("user32.dll")] public static extern bool SetWindowPos(IntPtr h, IntPtr after, int x, int y, int cx, int cy, uint flags);
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left, Top, Right, Bottom; }

  public static List<IntPtr> TerminalWindows() {
    var res = new List<IntPtr>();
    EnumWindows((h, l) => {
      var c = new StringBuilder(256); GetClassName(h, c, 256);
      if (c.ToString() == "CASCADIA_HOSTING_WINDOW_CLASS") res.Add(h);
      return true; }, IntPtr.Zero);
    return res;
  }
  public static string Title(IntPtr h) { var s = new StringBuilder(512); GetWindowText(h, s, 512); return s.ToString(); }

  public static Bitmap Print(IntPtr h) {
    RECT r; GetWindowRect(h, out r);
    int w = r.Right - r.Left, ht = r.Bottom - r.Top;
    var bmp = new Bitmap(w, ht);
    using (var g = Graphics.FromImage(bmp)) {
      IntPtr dc = g.GetHdc();
      bool ok = PrintWindow(h, dc, 2); // PW_RENDERFULLCONTENT
      g.ReleaseHdc(dc);
      if (!ok) { bmp.Dispose(); return null; }
    }
    return bmp;
  }
  public static Bitmap Screen(IntPtr h) {
    RECT r; GetWindowRect(h, out r);
    var bmp = new Bitmap(r.Right - r.Left, r.Bottom - r.Top);
    using (var g = Graphics.FromImage(bmp)) g.CopyFromScreen(r.Left, r.Top, 0, 0, bmp.Size);
    return bmp;
  }
  // Une image PrintWindow ratee est uniforme (noire ou transparente) : on la detecte.
  public static bool LooksBlank(Bitmap b) {
    var first = b.GetPixel(b.Width / 2, b.Height / 2);
    for (int y = b.Height / 8; y < b.Height; y += Math.Max(1, b.Height / 40))
      for (int x = b.Width / 8; x < b.Width; x += Math.Max(1, b.Width / 60))
        if (b.GetPixel(x, y) != first) return false;
    return true;
  }
}
'@
}
[void][CapWin]::SetProcessDPIAware()

function Write-Step([string]$msg) { Write-Host ("[{0:HH:mm:ss}] {1}" -f (Get-Date), $msg) }

# --- Fenetre --------------------------------------------------------------
function Find-Window([string]$title, [int]$timeoutMs = 20000) {
  $sw = [Diagnostics.Stopwatch]::StartNew()
  while ($sw.ElapsedMilliseconds -lt $timeoutMs) {
    foreach ($h in [CapWin]::TerminalWindows()) {
      if ([CapWin]::Title($h) -eq $title) { return $h }
    }
    Start-Sleep -Milliseconds 250
  }
  throw "Fenetre '$title' introuvable apres $timeoutMs ms"
}

function Focus-Window([IntPtr]$h) {
  if ([CapWin]::GetForegroundWindow() -eq $h) { return $true }
  [void][CapWin]::ShowWindow($h, 9) # SW_RESTORE
  # Astuce ALT : autorise SetForegroundWindow depuis un processus d'arriere-plan.
  [CapWin]::keybd_event(0x12, 0, 0, [UIntPtr]::Zero)
  [CapWin]::keybd_event(0x12, 0, 2, [UIntPtr]::Zero)
  [void][CapWin]::SetForegroundWindow($h)
  Start-Sleep -Milliseconds 300
  return ([CapWin]::GetForegroundWindow() -eq $h)
}

function Send-Keys([IntPtr]$h, [string]$keys) {
  # On n'envoie JAMAIS de touche si notre fenetre n'est pas au premier plan.
  for ($i = 0; $i -lt 5; $i++) {
    if (Focus-Window $h) {
      [System.Windows.Forms.SendKeys]::SendWait($keys)
      return
    }
    Start-Sleep -Milliseconds 400
  }
  throw "Impossible de mettre la fenetre au premier plan : touches '$keys' non envoyees"
}

function Escape-SendKeys([string]$text) {
  # Caracteres speciaux de SendKeys : + ^ % ~ ( ) { } [ ]
  return ([regex]::Replace($text, '[+^%~(){}\[\]]', { param($m) '{' + $m.Value + '}' }))
}

# --- Texte visible (UI Automation) -----------------------------------------
function Get-VisibleText([IntPtr]$h) {
  try {
    $root = [System.Windows.Automation.AutomationElement]::FromHandle($h)
    $cond = New-Object System.Windows.Automation.PropertyCondition(
      [System.Windows.Automation.AutomationElement]::ClassNameProperty, 'TermControl')
    $el = $root.FindFirst([System.Windows.Automation.TreeScope]::Descendants, $cond)
    if (-not $el) { return '' }
    $tp = $el.GetCurrentPattern([System.Windows.Automation.TextPattern]::Pattern)
    $ranges = $tp.GetVisibleRanges()
    $sb = New-Object System.Text.StringBuilder
    foreach ($r in $ranges) { [void]$sb.Append($r.GetText(-1)) }
    return $sb.ToString()
  } catch { return '' }
}

function Wait-Text([IntPtr]$h, [string]$pattern, [int]$timeoutMs) {
  $sw = [Diagnostics.Stopwatch]::StartNew()
  while ($sw.ElapsedMilliseconds -lt $timeoutMs) {
    if ((Get-VisibleText $h) -match $pattern) { return $true }
    Start-Sleep -Milliseconds 500
  }
  Write-Warning "Texte /$pattern/ non vu apres $timeoutMs ms"
  return $false
}

function Wait-Stop([string]$doneFile, [int]$count, [int]$timeoutMs) {
  $sw = [Diagnostics.Stopwatch]::StartNew()
  while ($sw.ElapsedMilliseconds -lt $timeoutMs) {
    if ((Test-Path $doneFile) -and (@(Get-Content $doneFile).Count -ge $count)) { return $true }
    Start-Sleep -Milliseconds 500
  }
  Write-Warning "Fin de tour n$count non vue apres $timeoutMs ms"
  return $false
}

# Largeur en colonnes du texte visible (longueur des lignes du tampon).
function Get-Columns([IntPtr]$h) {
  $lines = @((Get-VisibleText $h) -split "`r?`n" | Where-Object { $_.Length -gt 0 })
  if (-not $lines.Count) { return 0 }
  return ($lines | ForEach-Object { $_.Length } | Group-Object | Sort-Object Count -Descending | Select-Object -First 1).Name -as [int]
}

# wt --size ne donne pas toujours le bon nombre de colonnes (mise a l'echelle DPI) :
# on corrige la largeur de la fenetre jusqu'a obtenir exactement $cols colonnes.
function Set-Columns([IntPtr]$h, [int]$cols) {
  for ($i = 0; $i -lt 6; $i++) {
    $sw = [Diagnostics.Stopwatch]::StartNew()
    $have = 0
    while ($sw.ElapsedMilliseconds -lt 10000 -and -not $have) { $have = Get-Columns $h; if (-not $have) { Start-Sleep -Milliseconds 300 } }
    if ($have -eq $cols) { return $true }
    $r = New-Object CapWin+RECT
    [void][CapWin]::GetWindowRect($h, [ref]$r)
    $w = $r.Right - $r.Left
    $charW = [Math]::Max(4.0, ($w - 30) / $have)
    $neww = [int][Math]::Round($w - ($have - $cols) * $charW)
    [void][CapWin]::SetWindowPos($h, [IntPtr]::Zero, 0, 0, $neww, ($r.Bottom - $r.Top), 0x0016) # NOMOVE|NOZORDER|NOACTIVATE
    Start-Sleep -Milliseconds 700
  }
  Write-Warning "Largeur de $cols colonnes non atteinte (actuelle : $(Get-Columns $h))"
  return $false
}

# --- Capture --------------------------------------------------------------
function Save-Capture([IntPtr]$h, [string]$base) {
  $bmp = [CapWin]::Print($h)
  $method = 'PrintWindow'
  if (-not $bmp -or [CapWin]::LooksBlank($bmp)) {
    if ($bmp) { $bmp.Dispose() }
    [void](Focus-Window $h)
    Start-Sleep -Milliseconds 300
    $bmp = [CapWin]::Screen($h)
    $method = 'CopyFromScreen'
  }
  $bmp.Save("$base.png", [System.Drawing.Imaging.ImageFormat]::Png)
  $bmp.Dispose()
  $text = Get-VisibleText $h
  Set-Content -Encoding utf8 -Path "$base.uia.txt" -Value $text
  if ($text -match 'using the classic renderer') {
    Write-Warning "  $([IO.Path]::GetFileName($base)) : le CLI est retombe sur le rendu classique (capture douteuse)"
  }
  Write-Step "  capture $([IO.Path]::GetFileName($base)).png ($method)"
}

# --- Processus ------------------------------------------------------------
function Stop-ScenarioProcesses([string]$marker) {
  # cmd.exe qui execute le run.cmd de CE scenario, et toute sa descendance.
  $all = Get-CimInstance Win32_Process
  $roots = @($all | Where-Object { $_.Name -eq 'cmd.exe' -and $_.CommandLine -and $_.CommandLine.Contains($marker) })
  $kill = New-Object System.Collections.Generic.List[int]
  $queue = New-Object System.Collections.Generic.Queue[int]
  foreach ($r in $roots) { $queue.Enqueue([int]$r.ProcessId) }
  while ($queue.Count) {
    $p = $queue.Dequeue(); $kill.Add($p)
    foreach ($c in $all | Where-Object { $_.ParentProcessId -eq $p }) { $queue.Enqueue([int]$c.ProcessId) }
  }
  foreach ($p in ($kill | Sort-Object -Descending)) { try { Stop-Process -Id $p -Force -ErrorAction Stop } catch {} }
}

# --- Scenario -------------------------------------------------------------
function Invoke-Scenario($s, [string[]]$wanted) {
  $dir = Join-Path $SandboxRoot $s.name
  if (Test-Path $dir) { Remove-Item -Recurse -Force $dir }
  New-Item -ItemType Directory -Force $dir | Out-Null
  if ($s.PSObject.Properties['files']) {
    foreach ($f in $s.files) {
      Copy-Item (Join-Path $PSScriptRoot "files\$f") (Join-Path $dir $f)
    }
  }
  $doneFile = Join-Path $dir '.turn-done'
  $hookCmd = "node -e ""require('fs').appendFileSync('" + ($doneFile -replace '\\', '/') + "','stop\n')"""
  $settings = [ordered]@{
    theme       = $Config.theme
    tui         = $Config.tui
    hooks       = @{ Stop = @(@{ hooks = @(@{ type = 'command'; command = $hookCmd }) }) }
    permissions = @{ allow = @($Config.allow) }
  }
  $settingsFile = Join-Path $dir '.claude-ref-settings.json'
  $settings | ConvertTo-Json -Depth 10 | Set-Content -Encoding utf8 $settingsFile

  $argsLine = @('--model', $Config.model, '--settings', ('"' + $settingsFile + '"'))
  if ($s.PSObject.Properties['args']) { $argsLine += $s.args }
  if ($s.PSObject.Properties['prompt'] -and $s.prompt) {
    $argsLine += ('"' + ($s.prompt -replace '"', '\"') + '"')
  }
  $run = Join-Path $dir 'run.cmd'
  # chcp 65001 : le prompt passe en argument contient des accents.
  # wt transmet l'environnement de l'appelant : si ce script tourne lui-meme sous
  # Claude Code, on efface ses variables (sinon « Transcript saving is off », NO_COLOR, etc.).
  $unset = @(Get-ChildItem env: | Where-Object { $_.Name -match '^(CLAUDE|ANTHROPIC_|NO_COLOR$|FORCE_COLOR$|TERM$|COLORTERM$|CI$)' } | ForEach-Object { 'set ' + $_.Name + '=' })
  Set-Content -Encoding utf8 $run -Value (@('@echo off', 'chcp 65001 >nul') + $unset + @(
    ('"' + $Claude + '" ' + ($argsLine -join ' '))))

  $title = "claude-ref-" + $s.name + "-" + (Get-Random -Maximum 999999)
  $wtArgs = @('-w', 'new', '--size', "$($Config.cols),$($Config.rows)", '--pos', '40,40',
    'new-tab', '-p', $Config.wtProfile, '--colorScheme', $Config.colorScheme,
    '--title', $title, '--suppressApplicationTitle', '-d', $dir, 'cmd.exe', '/c', $run)
  Write-Step "Scenario $($s.name) : ouverture de la fenetre $title"
  # Start-Process ne met pas de guillemets autour des arguments : on le fait ici.
  $wtLine = ($wtArgs | ForEach-Object { if ($_ -match '\s') { '"' + $_ + '"' } else { $_ } }) -join ' '
  Start-Process wt.exe -ArgumentList $wtLine
  $h = [IntPtr]::Zero
  try {
    $h = Find-Window $title
    [void](Set-Columns $h ([int]$Config.cols))
    foreach ($st in $s.steps) {
      switch ($st.do) {
        'wait' { Start-Sleep -Milliseconds ([int]$st.ms) }
        'waitText' {
          $t = if ($st.PSObject.Properties['timeoutMs']) { [int]$st.timeoutMs } else { 60000 }
          [void](Wait-Text $h $st.pattern $t)
        }
        'waitStop' {
          $t = if ($st.PSObject.Properties['timeoutMs']) { [int]$st.timeoutMs } else { 180000 }
          [void](Wait-Stop $doneFile ([int]$st.count) $t)
        }
        'keys' { Send-Keys $h $st.send; Write-Step "  touches $($st.send)" }
        'type' { Send-Keys $h (Escape-SendKeys $st.text) }
        'capture' {
          $id = [string]$st.id
          if (-not $wanted -or @($wanted | Where-Object { $id.StartsWith($_) }).Count) {
            Save-Capture $h (Join-Path $OutDir ($st.id + '-' + $st.name))
          }
        }
        default { throw "Etape inconnue : $($st.do)" }
      }
    }
  } finally {
    if ($h -ne [IntPtr]::Zero) { [void][CapWin]::PostMessage($h, 0x0010, [IntPtr]::Zero, [IntPtr]::Zero) } # WM_CLOSE
    Start-Sleep -Milliseconds 1500
    Stop-ScenarioProcesses $run
    Write-Step "Scenario $($s.name) : fenetre fermee"
  }
}

New-Item -ItemType Directory -Force $OutDir, $SandboxRoot | Out-Null
$todo = @($Config.scenarios)
$wanted = @()
if ($Only) {
  $wanted = @($Only | ForEach-Object { $_ -split ',' } | ForEach-Object { if ($_ -match '^\d+$') { '{0:D2}' -f [int]$_ } else { $_ } })
  $todo = @($todo | Where-Object {
      @($_.steps | Where-Object { $c = $_; $_.do -eq 'capture' -and @($wanted | Where-Object { ([string]$c.id).StartsWith($_) }).Count }).Count })
} elseif ($Scenario) {
  $todo = @($todo | Where-Object { $Scenario -contains $_.name })
} else {
  $todo = @($todo | Where-Object { -not ($_.PSObject.Properties['extra'] -and $_.extra) })
}
if (-not $todo.Count) { throw 'Aucun scenario ne correspond' }
foreach ($s in $todo) { Invoke-Scenario $s $wanted }
