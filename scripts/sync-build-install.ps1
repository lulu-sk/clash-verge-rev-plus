[CmdletBinding()]
param(
  [string]$Branch = "feature/proxy-speed-test",
  [string]$UpstreamUrl = "https://github.com/clash-verge-rev/clash-verge-rev.git",
  [string]$UpstreamBranch = "dev",
  [switch]$FullRelease,
  [switch]$SkipInstall,
  [switch]$SkipPush
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

$OutputEncoding = [Console]::InputEncoding = [Console]::OutputEncoding = New-Object System.Text.UTF8Encoding
$Script:RepoRoot = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot "..")).Path

<#
.SYNOPSIS
输出当前步骤标题。
#>
function Write-Step {
  param([Parameter(Mandatory)][string]$Message)

  Write-Host ""
  Write-Host "==> $Message" -ForegroundColor Cyan
}

<#
.SYNOPSIS
输出成功提示。
#>
function Write-Success {
  param([Parameter(Mandatory)][string]$Message)

  Write-Host "[OK] $Message" -ForegroundColor Green
}

<#
.SYNOPSIS
输出普通提示。
#>
function Write-Info {
  param([Parameter(Mandatory)][string]$Message)

  Write-Host "[INFO] $Message"
}

<#
.SYNOPSIS
查找必须存在的命令。
#>
function Get-RequiredCommand {
  param([Parameter(Mandatory)][string]$Name)

  $command = Get-Command $Name -ErrorAction SilentlyContinue
  if (-not $command) {
    throw "未找到命令：$Name。请先安装或把它加入 PATH。"
  }

  return $command.Source
}

<#
.SYNOPSIS
执行外部命令并设置超时时间。
#>
function Invoke-External {
  param(
    [Parameter(Mandatory)][string]$FilePath,
    [Parameter(Mandatory)][string[]]$ArgumentList,
    [int]$TimeoutSeconds = 600
  )

  $display = "$FilePath $($ArgumentList -join ' ')"
  Write-Host "> $display" -ForegroundColor DarkGray

  $process = Start-Process -FilePath $FilePath -ArgumentList $ArgumentList -NoNewWindow -PassThru
  if (-not $process.WaitForExit($TimeoutSeconds * 1000)) {
    try {
      $process.Kill($true)
    } catch {
      $process.Kill()
    }
    throw "命令超时：$display"
  }

  if ($process.ExitCode -ne 0) {
    throw "命令执行失败，退出码 $($process.ExitCode)：$display"
  }
}

<#
.SYNOPSIS
确认脚本运行在目标仓库根目录。
#>
function Assert-RepoRoot {
  Set-Location -LiteralPath $Script:RepoRoot

  if (-not (Test-Path -LiteralPath ".git")) {
    throw "当前目录不是 Git 仓库：$Script:RepoRoot"
  }

  if (-not (Test-Path -LiteralPath "package.json") -or -not (Test-Path -LiteralPath "src-tauri")) {
    throw "当前目录不像 Clash Verge Rev 仓库：$Script:RepoRoot"
  }

  Write-Info "仓库目录：$Script:RepoRoot"
}

<#
.SYNOPSIS
读取已跟踪文件的工作区状态。
#>
function Get-TrackedWorktreeStatus {
  param([Parameter(Mandatory)][string]$Git)

  $status = & $Git status --porcelain=v1 --untracked-files=no
  if ($LASTEXITCODE -ne 0) {
    throw "无法读取 Git 状态。"
  }

  return @($status)
}

<#
.SYNOPSIS
读取当前 Git 分支名称。
#>
function Get-CurrentBranch {
  param([Parameter(Mandatory)][string]$Git)

  $branch = & $Git branch --show-current
  if ($LASTEXITCODE -ne 0) {
    throw "无法读取当前 Git 分支。"
  }

  return $branch.Trim()
}

<#
.SYNOPSIS
确认仓库没有未解决冲突。
#>
function Assert-NoUnmergedChanges {
  param([Parameter(Mandatory)][string]$Git)

  if (Test-UnmergedChanges -Git $Git) {
    throw "当前仓库存在未解决冲突。请先处理冲突后再运行。"
  }
}

<#
.SYNOPSIS
检查仓库是否存在未解决冲突。
#>
function Test-UnmergedChanges {
  param([Parameter(Mandatory)][string]$Git)

  $unmerged = @(Get-TrackedWorktreeStatus -Git $Git | Where-Object {
      $_ -match "^(DD|AU|UD|UA|DU|AA|UU) "
    })

  return [bool]$unmerged
}

<#
.SYNOPSIS
同步前临时保存已跟踪文件的本地改动。
#>
function Save-TrackedWorktreeChanges {
  param([Parameter(Mandatory)][string]$Git)

  Assert-NoUnmergedChanges -Git $Git

  $status = @(Get-TrackedWorktreeStatus -Git $Git)
  if (-not $status) {
    return $null
  }

  $currentBranch = Get-CurrentBranch -Git $Git
  if ($currentBranch -ne $Branch) {
    throw "当前分支为 $currentBranch，且存在已跟踪文件的本地改动。请先切回 $Branch 或手动处理这些改动。"
  }

  $stashBefore = & $Git rev-parse -q --verify refs/stash 2>$null
  if ($LASTEXITCODE -ne 0) {
    $stashBeforeId = ""
  } else {
    $stashBeforeId = $stashBefore.Trim()
  }

  $stashMessage = "sync-build-install auto stash $(Get-Date -Format "yyyyMMdd-HHmmss")"
  Write-Info "检测到已跟踪文件有本地改动，同步前临时保存到 Git stash。"

  $stashOutput = & $Git stash push --message $stashMessage 2>&1
  if ($LASTEXITCODE -ne 0) {
    throw "保存本地改动失败：$($stashOutput -join [Environment]::NewLine)"
  }

  $stashAfter = & $Git rev-parse -q --verify refs/stash 2>$null
  if ($LASTEXITCODE -ne 0) {
    return $null
  }

  if ($stashAfter.Trim() -eq $stashBeforeId) {
    return $null
  }

  return "stash@{0}"
}

<#
.SYNOPSIS
恢复同步前临时保存的本地改动。
#>
function Restore-TrackedWorktreeChanges {
  param(
    [Parameter(Mandatory)][string]$Git,
    [string]$StashRef
  )

  if ([string]::IsNullOrWhiteSpace($StashRef)) {
    return
  }

  Write-Info "恢复同步前临时保存的本地改动。"
  Invoke-External -FilePath $Git -ArgumentList @("stash", "pop", "--index", $StashRef) -TimeoutSeconds 600
}

<#
.SYNOPSIS
确保官方 upstream 远端存在且地址正确。
#>
function Ensure-UpstreamRemote {
  param([Parameter(Mandatory)][string]$Git)

  $currentUrl = & $Git remote get-url upstream 2>$null
  if ($LASTEXITCODE -ne 0) {
    Invoke-External -FilePath $Git -ArgumentList @("remote", "add", "upstream", $UpstreamUrl) -TimeoutSeconds 60
    return
  }

  if ($currentUrl.Trim() -ne $UpstreamUrl) {
    Write-Info "修正 upstream 地址：$UpstreamUrl"
    Invoke-External -FilePath $Git -ArgumentList @("remote", "set-url", "upstream", $UpstreamUrl) -TimeoutSeconds 60
  }
}

<#
.SYNOPSIS
切换到个人功能分支。
#>
function Switch-FeatureBranch {
  param([Parameter(Mandatory)][string]$Git)

  & $Git rev-parse --verify $Branch *> $null
  if ($LASTEXITCODE -eq 0) {
    Invoke-External -FilePath $Git -ArgumentList @("switch", $Branch) -TimeoutSeconds 60
    return
  }

  & $Git rev-parse --verify "origin/$Branch" *> $null
  if ($LASTEXITCODE -eq 0) {
    Invoke-External -FilePath $Git -ArgumentList @("switch", "-c", $Branch, "--track", "origin/$Branch") -TimeoutSeconds 60
    return
  }

  throw "未找到分支：$Branch"
}

<#
.SYNOPSIS
同步官方 dev 到个人功能分支。
#>
function Sync-FromUpstream {
  param([Parameter(Mandatory)][string]$Git)

  $stashRef = Save-TrackedWorktreeChanges -Git $Git

  try {
    Ensure-UpstreamRemote -Git $Git

    Invoke-External -FilePath $Git -ArgumentList @("fetch", "origin", "--prune") -TimeoutSeconds 600
    Invoke-External -FilePath $Git -ArgumentList @("fetch", "upstream", $UpstreamBranch, "--tags") -TimeoutSeconds 600

    Switch-FeatureBranch -Git $Git
    Assert-NoUnmergedChanges -Git $Git

    Invoke-External -FilePath $Git -ArgumentList @("merge", "--no-edit", "upstream/$UpstreamBranch") -TimeoutSeconds 600

    if (-not $SkipPush) {
      Invoke-External -FilePath $Git -ArgumentList @("push", "origin", $Branch) -TimeoutSeconds 600
    }
  } finally {
    if ($stashRef) {
      if (Test-UnmergedChanges -Git $Git) {
        Write-Host "[警告] 同步产生未解决冲突，已保留临时 stash：$stashRef。处理冲突后可运行：git stash pop --index $stashRef" -ForegroundColor Yellow
      } else {
        Restore-TrackedWorktreeChanges -Git $Git -StashRef $stashRef
      }
    }
  }
}

<#
.SYNOPSIS
确保 pnpm 可用。
#>
function Get-PnpmCommand {
  $pnpm = Get-Command pnpm.cmd -ErrorAction SilentlyContinue
  if ($pnpm) {
    return $pnpm.Source
  }

  $pnpm = Get-Command pnpm -ErrorAction SilentlyContinue
  if ($pnpm) {
    return $pnpm.Source
  }

  $corepack = Get-Command corepack -ErrorAction SilentlyContinue
  if ($corepack) {
    Invoke-External -FilePath $corepack.Source -ArgumentList @("enable") -TimeoutSeconds 300
    $pnpm = Get-Command pnpm.cmd -ErrorAction SilentlyContinue
    if ($pnpm) {
      return $pnpm.Source
    }

    $pnpm = Get-Command pnpm -ErrorAction SilentlyContinue
    if ($pnpm) {
      return $pnpm.Source
    }
  }

  throw "未找到 pnpm。请先安装 Node.js，并启用 corepack 或安装 pnpm。"
}

<#
.SYNOPSIS
安装或更新前端依赖。
#>
function Install-Dependencies {
  param([Parameter(Mandatory)][string]$Pnpm)

  Invoke-External -FilePath $Pnpm -ArgumentList @("install", "--frozen-lockfile") -TimeoutSeconds 1800
}

<#
.SYNOPSIS
删除旧的 Windows 服务资源。
#>
function Remove-ServiceBuildResources {
  $resourceDir = Join-Path $Script:RepoRoot "src-tauri\resources"
  if (-not (Test-Path -LiteralPath $resourceDir)) {
    return
  }

  Get-ChildItem -LiteralPath $resourceDir -File -Filter "clash-verge-service*.exe" |
    ForEach-Object {
      Write-Info "删除旧服务资源：$($_.Name)"
      Remove-Item -LiteralPath $_.FullName -Force
    }
}

<#
.SYNOPSIS
刷新打包所需的服务资源。
#>
function Update-BuildResources {
  param([Parameter(Mandatory)][string]$Pnpm)

  Remove-ServiceBuildResources
  Invoke-External -FilePath $Pnpm -ArgumentList @("run", "prebuild") -TimeoutSeconds 3600
}

<#
.SYNOPSIS
构建本地安装包。
#>
function Build-Installer {
  param([Parameter(Mandatory)][string]$Pnpm)

  $previousNodeOptions = $env:NODE_OPTIONS
  $env:NODE_OPTIONS = "--max-old-space-size=4096"

  $configPath = $null
  $arguments = @("tauri", "build")
  if ([string]::IsNullOrWhiteSpace($env:TAURI_SIGNING_PRIVATE_KEY)) {
    $configPath = New-LocalBuildConfig
    Write-Info "未检测到 TAURI_SIGNING_PRIVATE_KEY，本地构建将跳过 updater 签名产物。"
    $arguments += @("--config", $configPath)
  }

  if (-not $FullRelease) {
    $arguments += @("--", "--profile", "fast-release")
  }

  try {
    Invoke-External -FilePath $Pnpm -ArgumentList $arguments -TimeoutSeconds 7200
  } finally {
    $env:NODE_OPTIONS = $previousNodeOptions
    if ($configPath -and (Test-Path -LiteralPath $configPath)) {
      Remove-Item -LiteralPath $configPath -Force
    }
  }
}

<#
.SYNOPSIS
创建本地构建使用的 Tauri 临时覆盖配置。
#>
function New-LocalBuildConfig {
  $configPath = Join-Path ([System.IO.Path]::GetTempPath()) "clash-verge-local-build-$([Guid]::NewGuid().ToString("N")).json"
  $config = @{
    bundle = @{
      createUpdaterArtifacts = $false
    }
  }

  $config |
    ConvertTo-Json -Depth 4 |
    Set-Content -LiteralPath $configPath -Encoding utf8NoBOM

  return $configPath
}

<#
.SYNOPSIS
查找最新生成的 Windows 安装包。
#>
function Find-LatestInstaller {
  $targetRoots = @(
    (Join-Path $Script:RepoRoot "target"),
    (Join-Path $Script:RepoRoot "src-tauri\target")
  ) | Where-Object { Test-Path -LiteralPath $_ }

  if (-not $targetRoots) {
    throw "未找到构建目录：$(Join-Path $Script:RepoRoot "target") 或 $(Join-Path $Script:RepoRoot "src-tauri\target")"
  }

  $installers = foreach ($targetRoot in $targetRoots) {
    Get-ChildItem -LiteralPath $targetRoot -Recurse -File -ErrorAction SilentlyContinue |
      Where-Object {
        $_.FullName -like "*\bundle\*" -and
        $_.Extension -in @(".exe", ".msi", ".msix")
      }
  }

  if (-not $installers) {
    throw "没有找到安装包。请查看上方构建日志。"
  }

  return $installers |
    Sort-Object `
      @{ Expression = {
          if ($_.Extension -eq ".exe") { 0 }
          elseif ($_.Extension -eq ".msi") { 1 }
          else { 2 }
        }; Ascending = $true },
      @{ Expression = { $_.LastWriteTime }; Descending = $true } |
    Select-Object -First 1
}

<#
.SYNOPSIS
输出当前 Git 状态，便于失败时排查。
#>
function Write-GitStatus {
  param([Parameter(Mandatory)][string]$Git)

  Write-Host ""
  Write-Host "当前 Git 状态：" -ForegroundColor Yellow
  & $Git status --short --branch
}

try {
  Assert-RepoRoot

  $git = Get-RequiredCommand -Name "git"

  Write-Step "同步官方更新"
  Sync-FromUpstream -Git $git

  Write-Step "安装依赖"
  $pnpm = Get-PnpmCommand
  Install-Dependencies -Pnpm $pnpm

  Write-Step "刷新构建资源"
  Update-BuildResources -Pnpm $pnpm

  Write-Step "构建安装包"
  Build-Installer -Pnpm $pnpm

  Write-Step "定位安装包"
  $installer = Find-LatestInstaller
  Write-Success "安装包：$($installer.FullName)"

  if (-not $SkipInstall) {
    Write-Info "正在打开安装包，请按安装器提示完成安装。"
    Start-Process -FilePath $installer.FullName
  } else {
    Write-Info "已跳过自动打开安装包。"
  }

  Write-Success "流程完成。"
  exit 0
} catch {
  Write-Host ""
  Write-Host "[失败] $($_.Exception.Message)" -ForegroundColor Red

  $gitForStatus = Get-Command git -ErrorAction SilentlyContinue
  if ($gitForStatus -and (Test-Path -LiteralPath (Join-Path $Script:RepoRoot ".git"))) {
    Set-Location -LiteralPath $Script:RepoRoot
    Write-GitStatus -Git $gitForStatus.Source
  }

  Write-Host ""
  Write-Host "如果失败原因是 merge conflict，请把窗口内容发给我，我来处理冲突。" -ForegroundColor Yellow
  exit 1
}
