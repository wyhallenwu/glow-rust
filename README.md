# Glow（Rust）

Glow 是一个面向终端和浏览器的 Markdown 阅读器。它可以渲染单个文件，递归浏览某个目录下的全部 Markdown 文档，把目录启动为只读文档网站，也可以通过 Cloudflare Quick Tunnel 临时分享至公网。

本项目已使用 Rust 重写。终端 UI、Web UI、LaTeX、Mermaid、目录监听与 Cloudflare Tunnel 生命周期都由一个二进制提供；运行 Web 阅读器不需要 Node.js，也不需要从 CDN 下载前端依赖。

## 目录

- [主要功能](#主要功能)
- [平台与构建要求](#平台与构建要求)
- [一键构建脚本](#一键构建脚本)
- [快速开始](#快速开始)
- [输入来源与工作模式](#输入来源与工作模式)
- [目录扫描规则](#目录扫描规则)
- [终端浏览器](#终端浏览器)
- [Markdown、LaTeX 与图表](#markdownlatex-与图表)
- [Web 文档站](#web-文档站)
- [Cloudflare 公网分享](#cloudflare-公网分享)
- [完整 CLI 参考](#完整-cli-参考)
- [配置文件与环境变量](#配置文件与环境变量)
- [Shell 补全](#shell-补全)
- [安全模型与限制](#安全模型与限制)
- [故障排查](#故障排查)
- [开发与测试](#开发与测试)

## 主要功能

- 递归识别 `.md`、`.markdown`、`.mdown`、`.mkd` 和 `.mkdn` 文件，不区分扩展名大小写。
- 默认忽略隐藏文件以及 `.gitignore`、全局 gitignore 和 `.git/info/exclude` 中排除的内容；需要时可使用 `--all`。
- 分栏式终端 UI，包含可折叠目录树、即时过滤、文档预览、文件变化监听、窄终端布局和可选鼠标操作。
- 终端支持 Unicode/CJK 宽度计算、表格、任务列表、脚注、引用、链接、代码高亮和代码行号。
- Web 阅读器包含响应式导航、全文档列表过滤、页内目录、明暗主题、相对链接、本地图片/PDF 和自动刷新。
- Web 端将 LaTeX 渲染为原生 MathML，并将 Mermaid 图表预渲染为明暗双主题 SVG。
- 支持本地文件、标准输入、HTTP(S) URL，以及 GitHub/GitLab 仓库 README 快捷方式。
- 使用 `glow share` 一条命令创建临时 Cloudflare Quick Tunnel。
- Web 静态资源直接嵌入 Rust 二进制，发布时只需要一个 `glow` 文件。

## 平台与构建要求

仓库自带的构建脚本支持：

- macOS（Apple Silicon 或 Intel）
- Linux（常用 Rust GNU target 架构）
- Bash 3.2 或更高版本
- Rust 1.88 或更高版本；没有 Rust 时脚本会通过官方 `rustup` 安装当前 stable minimal toolchain

Rust 自身仍需要系统链接器。构建脚本不会调用 `sudo`，也不会擅自安装系统软件包。

macOS 如果尚未安装 Command Line Tools，请先执行：

```bash
xcode-select --install
```

Debian/Ubuntu 的最小系统通常需要：

```bash
sudo apt-get update
sudo apt-get install -y build-essential curl ca-certificates
```

Fedora/RHEL、Arch、Alpine 等发行版需要各自对应的 C 编译器、链接器、CA 证书以及 `curl` 或 `wget`。Glow 本身使用 Rustls，不要求系统 OpenSSL 开发包。

## 一键构建脚本

在仓库中执行：

```bash
bash scripts/build.sh
```

脚本会：

1. 根据脚本自身位置找到仓库，因此可以从任意工作目录调用。
2. 检测 macOS/Linux、CPU 架构和系统链接器。
3. 检查 Rust 版本；如果没有 Rust，则从 `https://sh.rustup.rs` 非交互安装 stable minimal toolchain。
4. 使用锁定的 `Cargo.lock` 执行 `cargo build --release --locked`。
5. 默认把二进制安装到 `${PREFIX}/bin/glow`；未设置 `PREFIX` 时使用 `~/.local/bin/glow`。

脚本不使用 `sudo`，Rust 安装使用 `--no-modify-path`，不会自动修改 `.zshrc`、`.bashrc` 等 shell 配置。

### 构建脚本参数

| 参数 | 作用 |
| --- | --- |
| `--no-install` | 只构建，不复制二进制 |
| `--install-dir <DIR>` | 直接安装到指定目录 |
| `--prefix <PREFIX>` | 安装到 `<PREFIX>/bin` |
| `--target <TRIPLE>` | 构建指定 Rust target；有 rustup 时自动添加其标准库 |
| `-h`, `--help` | 显示脚本帮助 |

示例：

```bash
# 仅生成 target/release/glow
bash scripts/build.sh --no-install

# 安装到当前仓库的 bin 目录
bash scripts/build.sh --install-dir ./bin

# 安装到自定义 prefix
bash scripts/build.sh --prefix "$HOME/apps/glow"

# 指定 Rust target
bash scripts/build.sh --target aarch64-unknown-linux-gnu --no-install
```

`PREFIX`、`CARGO_HOME` 和 `CARGO_TARGET_DIR` 环境变量会被尊重。例如：

```bash
PREFIX="$HOME/.local" CARGO_TARGET_DIR=/tmp/glow-target bash scripts/build.sh
```

指定非本机 `--target` 属于交叉编译。脚本可以安装 Rust 标准库，但不会安装目标平台的 linker、C runtime 或 sysroot；这些仍需通过交叉工具链提供。

默认安装和显式安装都会替换目标目录中已有的 `glow` 文件。只想验证构建或保留现有安装时，请使用 `--no-install`。

如果 `~/.local/bin` 不在 `PATH` 中，可加入当前 shell：

```bash
export PATH="$HOME/.local/bin:$PATH"
```

然后把同一行写入 `~/.zshrc` 或 `~/.bashrc`。

### 手动构建

不使用脚本时：

```bash
cargo build --release --locked
install -d "$HOME/.local/bin"
install -m 755 target/release/glow "$HOME/.local/bin/glow"
```

验证安装：

```bash
glow --version
glow --help
```

## 快速开始

```bash
# 递归浏览当前目录或指定 docs 目录
glow
glow ./docs

# 渲染一个文件
glow README.md

# 从标准输入渲染
printf '# Hello\n\n$E=mc^2$\n' | glow
cat guide.md | glow -

# 使用 pager
glow README.md --pager

# 读取远程文档或仓库 README
glow https://example.com/guide.md
glow github.com/owner/repository
glow github://owner/repository
glow gitlab://group/project

# 查看扫描结果
glow list ./docs
glow list ./docs --json

# 本地 Web 文档站
glow serve ./docs --open

# 临时公网分享
glow share ./docs --open
```

## 输入来源与工作模式

| 输入 | 行为 |
| --- | --- |
| 无参数，且终端没有管道输入 | 打开当前目录的交互式 TUI |
| 目录 | 递归扫描并打开 TUI |
| Markdown 文件 | 在当前终端渲染 |
| 其他文本文件 | 按扩展名包装成代码块后渲染 |
| `-` 或标准输入管道 | 读取 UTF-8 Markdown 并渲染 |
| `http://` / `https://` URL | 下载并渲染远程 UTF-8 文档 |
| GitHub/GitLab 仓库快捷方式 | 通过对应 API 获取仓库 README |

远程响应的最大体积为 10 MiB，请求超时为 20 秒。远程内容必须是 UTF-8。仓库快捷方式目前没有令牌参数，因此私有仓库通常无法读取。

当标准输入不是终端时，Glow 优先读取标准输入。自动化脚本中建议显式使用 `glow -`，让输入意图更清楚。

## 目录扫描规则

Glow 会递归扫描传入目录的全部子目录：

- 支持扩展名：`.md`、`.markdown`、`.mdown`、`.mkd`、`.mkdn`。
- 默认不扫描隐藏路径。
- 默认遵循仓库、父目录、全局 gitignore 和 `.git/info/exclude`。
- 不跟随目录或文件符号链接，避免扫描循环和越过文档根目录。
- `--all` 同时关闭隐藏路径过滤和 gitignore 过滤。
- 文档标题优先取 YAML frontmatter 之后遇到的第一个 Markdown 标题；没有标题时使用文件名。
- 默认选中根目录的 `README.md`、`README.markdown` 或 `README.mdown`；没有 README 时选择排序后的第一个文档。

扫描结果可以在启动 TUI 或 Web 服务之前检查：

```bash
glow list ./docs
glow list ./docs --json
glow --all list ./docs --json
```

JSON 项目包含 `path`、`title` 和字节数 `size`，适合接入脚本或 CI。

## 终端浏览器

打开目录会进入 TUI。宽度达到 88 列时显示目录树和预览双栏；更窄时一次显示一个面板，通过 `Tab` 或 `Enter` 切换。

目录变化由文件系统 watcher 递归监听，并在短暂 debounce 后自动刷新。也可以按 `r` 立即重新扫描。

### 快捷键

| 快捷键 | 目录树 | 预览 |
| --- | --- | --- |
| `↑` / `↓`、`j` / `k` | 移动选择 | 逐行滚动 |
| `Enter` | 展开/折叠目录；文档进入预览 | 保持预览焦点 |
| `→` | 展开目录；文档进入预览 | — |
| `←` | 折叠目录或选择父目录 | — |
| `Tab` / `Shift-Tab` | 在目录树和预览之间切换 | 同左 |
| `g` / `Home` | 第一项 | 文档顶部 |
| `G` / `End` | 最后一项 | 文档底部 |
| `PgUp` / `PgDn` | — | 按页滚动 |
| `/` | 输入路径/标题过滤条件 | 输入路径/标题过滤条件 |
| `Esc` | 清除过滤；没有过滤时退出 | 同左 |
| `r` | 立即重新扫描 | 立即重新扫描 |
| `?` | 打开/关闭帮助 | 打开/关闭帮助 |
| `q` / `Ctrl-C` | 退出 | 退出 |

在过滤输入期间，`Enter` 或 `Esc` 结束编辑，`Backspace` 删除字符；此时输入 `q` 会成为查询内容，而不会退出。

鼠标默认关闭，可以用隐藏 CLI 参数 `--mouse` 或在配置文件中设置 `mouse: true`。启用后支持点击面板/文档以及滚轮移动。

### 单文档终端输出

- `--width 0` 自动使用终端宽度，并将自动宽度上限设为 120；显式宽度最小为 8。
- `--pager` 使用 `$PAGER`，未设置时使用 `less -R`。
- `--style auto|dark|light` 控制配色。
- `--line-numbers` 给 fenced code block 增加行号。
- `--preserve-new-lines` 保留 Markdown soft break。
- 设置标准的 `NO_COLOR` 环境变量可关闭 ANSI 颜色。
- 输出被重定向到文件或管道时，Glow 自动关闭 ANSI 颜色。

## Markdown、LaTeX 与图表

终端和 Web 渲染器支持 CommonMark，并启用以下扩展：

- 表格
- 脚注
- 删除线
- 任务列表
- 智能标点
- fenced code block 语法高亮
- 行内与块级数学公式

文档开头的 YAML frontmatter 会从正文预览中移除。原始 HTML 不会作为可执行 HTML 注入：终端按内容处理，Web 端将其转义并对最终结果再次清洗。

### LaTeX

行内公式：

```markdown
质能方程为 $E = mc^2$。
```

块级公式：

```markdown
$$
\int_{-\infty}^{\infty} e^{-x^2}\,dx = \sqrt{\pi}
$$
```

Web 端通过纯 Rust 的 [`pulldown-latex`](https://github.com/carloskiki/pulldown-latex) 将公式转换为经过独立 allowlist 清洗的原生 MathML。常见分数、根式、上下标、极限、积分、重音、希腊字母、矩阵和对齐环境均可使用。超宽块级公式只在公式容器内部横向滚动，不会撑宽整页。

终端无法可靠排版 MathML，因此行内公式保留 `$…$`，块级公式使用独立边框卡片显示完整 TeX 源码。公式解析失败时，Web 端也保留经过转义的原始公式，而不是丢失内容。

### Mermaid 图表

使用 fenced `mermaid` block：

````markdown
```mermaid
flowchart LR
    Draft --> Review --> Publish
```
````

`mmd`、`mermaidjs`、`mermaid-js`、`language-mermaid` 和 `{.mermaid}` fence 标记也会被识别。

内置的纯 Rust [`mermaid-svg`](https://github.com/xmiksay/mermaid) renderer 覆盖流程图、时序图、类图、状态图、ER 图、Gantt、Journey、Timeline、饼图、XY、Quadrant、Radar、Sankey、Mindmap、Git graph、Requirement、C4、Block、Architecture、Packet、Kanban、Treemap 和 ZenUML 等图表类型。

Web 端为每张图预渲染亮色/暗色 SVG，并根据页面主题切换。生成的 SVG 被 Base64 编码后作为隔离的 `<img>` 加载，不会作为可执行 SVG DOM 注入页面。源码默认折叠但可以展开复制；空图、语法错误或超过 256 KiB 的图会显示错误和完整源码。

终端中 Mermaid 使用带行号和语法着色的源码卡片展示，保证即使终端不支持图形也能阅读定义。

纯 Rust renderer 与 Mermaid 官方 JavaScript 实现的版本和细节不一定完全一致。遇到兼容差异时，可以展开页面中的 Mermaid source 进行排查。

## Web 文档站

### 启动

```bash
# 默认监听 127.0.0.1，并由系统选择空闲端口
glow serve ./docs

# 启动后打开默认浏览器
glow serve ./docs --open

# 固定本机端口
glow serve ./docs --port 8080

# 允许局域网/容器外访问；使用前请阅读安全章节
glow serve ./docs --host 0.0.0.0 --port 8080
```

`--host` 只接受 IP 地址。`--port 0` 表示由操作系统分配空闲端口，这是默认行为。

Linux 上的 `--open` 使用 `xdg-open`，macOS 使用 `open`。无桌面环境的服务器应省略 `--open`，复制终端打印的 URL。

### 阅读体验

- 左侧文档导航和即时路径/标题过滤。
- 右侧根据 `h2`、`h3`、`h4` 自动生成页内目录。
- 自动跟随系统明暗主题，也可手动切换并保存在浏览器 local storage。
- 窄屏下使用可展开的移动端导航。
- 相对 Markdown 链接会改写为站内 `/doc/…` 路由。
- 相对图片/PDF 会改写为受控 `/asset/…` 路由。
- 服务端约每秒重新扫描目录，浏览器轮询小型状态接口；文件变化后页面自动刷新。

Web 服务只允许提供以下资产扩展名：

```text
avif bmp gif ico jpeg jpg png svg webp pdf
```

其他文件即使存在于文档根目录中也不会通过 `/asset/` 返回。

## Cloudflare 公网分享

`glow share` 在 loopback 上启动只读文档服务，再启动 `cloudflared` Quick Tunnel。Glow 不捆绑、下载或自动更新 `cloudflared`。

### 安装 cloudflared

macOS：

```bash
brew install cloudflared
cloudflared --version
```

Linux 请使用 Cloudflare 的[官方安装与下载说明](https://developers.cloudflare.com/tunnel/downloads/)，选择对应发行版的软件仓库或独立二进制，然后执行：

```bash
cloudflared --version
```

### 分享命令

```bash
# 打印本地 URL 和随机的 trycloudflare.com HTTPS URL
glow share ./docs

# 自动打开公网 URL
glow share ./docs --open

# 最多等待 60 秒获得公网 URL
glow share ./docs --timeout 60

# cloudflared 不在 PATH 时
glow share ./docs --cloudflared /absolute/path/to/cloudflared
CLOUDFLARED_BIN=/absolute/path/to/cloudflared glow share ./docs
```

Glow 在本地服务绑定成功后，直接启动以下等价参数，不通过 shell 拼接：

```text
cloudflared tunnel --no-autoupdate --url http://127.0.0.1:<系统分配端口>
```

`--timeout` 只控制等待 Cloudflare 输出公网 URL 的时间，不是 Tunnel 总运行时间，也不是 HTTP 请求超时。Glow 只接受严格的 `https://<单标签>.trycloudflare.com` origin，拒绝带用户信息、端口、路径、查询、fragment 或伪造后缀的 URL。

按 `Ctrl-C` 时，Glow 会依次停止本地服务和 `cloudflared` 并等待子进程退出。本地服务或 Tunnel 提前退出时，另一个进程也会在正常错误路径中被清理。`kill -9`、系统崩溃或断电无法运行清理逻辑。

> **公网安全警告：** Quick Tunnel 是匿名公网入口，Glow 没有登录、密码或 Cloudflare Access 鉴权。任何获得 URL 的人都可以访问。随机 hostname 不是访问控制，不要分享秘密、私人笔记或受合规限制的内容。

分享一个目录时，公网可请求范围包括：

- 扫描索引中的所有 Markdown，而不只是当前打开的文档。
- 扫描到的所有 allowlist 图片/PDF，即使它们没有被任何 Markdown 引用。
- 使用全局 `--all` 后，还包括隐藏以及被 gitignore 忽略的 Markdown 和允许资产。

因此执行下面的命令前必须先检查 `glow --all list ./docs` 的结果：

```bash
glow --all share ./docs
```

Cloudflare 将 Quick Tunnel 定位为测试/开发功能：随机 URL 不稳定、没有 SLA、限制 200 个 in-flight 请求、不支持 SSE；存在 `~/.cloudflared/config.yaml` 时当前也不支持 Quick Tunnel。完整限制以 [Cloudflare Quick Tunnel 官方文档](https://developers.cloudflare.com/cloudflare-one/networks/connectors/cloudflare-tunnel/do-more-with-tunnels/trycloudflare/)为准。

需要稳定域名、身份认证或生产发布时，应配置 named tunnel 与 Cloudflare Access；当前 `glow share` 只负责临时 Quick Tunnel。

## 完整 CLI 参考

```text
glow [OPTIONS] [SOURCE|DIR]
glow serve [PATH] [--host IP] [--port PORT] [--open]
glow share [PATH] [--cloudflared FILE] [--timeout SECONDS] [--open]
glow list [PATH] [--json]
glow config [path|init|edit|show]
glow completion <SHELL>
```

### 全局选项

| 选项 | 默认值 | 说明 |
| --- | --- | --- |
| `-p`, `--pager` | `false` | 使用 `$PAGER` 显示单文档输出 |
| `-t`, `--tui` | `false` | 对文件来源强制打开其父目录 TUI |
| `-s`, `--style <auto\|dark\|light>` | `auto` | 终端配色 |
| `-w`, `--width <N>` | `0` | 终端渲染宽度；`0` 自动检测 |
| `-a`, `--all` | `false` | 包含隐藏和 ignored 文档/资产 |
| `-l`, `--line-numbers` | `false` | fenced code block 行号 |
| `-n`, `--preserve-new-lines` | `false` | 保留 Markdown soft break |
| `--config <FILE>` | 自动查找 | 使用指定 YAML 配置 |
| `-h`, `--help` | — | 显示帮助 |
| `-V`, `--version` | — | 显示版本 |

`--pager` 与 `--tui` 不能同时启用。`--style`、`--width`、`--line-numbers`、`--preserve-new-lines` 主要影响终端渲染；Web 主题由浏览器端控制。`--all` 会影响 TUI、`list`、`serve` 和 `share` 的扫描范围。

### 子命令

#### `glow serve`

| 参数 | 默认值 | 说明 |
| --- | --- | --- |
| `[PATH]` | `.` | 文档根目录 |
| `--host <IP>` | `127.0.0.1` | 监听地址 |
| `-P`, `--port <PORT>` | `0` | 监听端口；`0` 自动分配 |
| `--open` | `false` | 打开默认浏览器 |

#### `glow share`

| 参数 | 默认值 | 说明 |
| --- | --- | --- |
| `[PATH]` | `.` | 文档根目录 |
| `--cloudflared <FILE>` | `cloudflared` | 可执行文件路径，也可用 `CLOUDFLARED_BIN` |
| `--timeout <SECONDS>` | `30` | 等待 Quick Tunnel URL 的时间 |
| `--open` | `false` | 打开公网 URL |

`share` 总是把 origin 强制绑定至 `127.0.0.1` 和系统空闲端口，不接受 LAN-facing origin。

#### `glow list`

```bash
glow list [PATH]
glow list [PATH] --json
```

默认输出 `<相对路径><TAB><标题>`；`--json` 输出结构化数组。

#### `glow config`

```bash
glow config path
glow config init
glow config init --force
glow config edit
glow config show
```

没有 action 的 `glow config` 等价于 `glow config edit`。`edit` 会在文件不存在时创建默认配置，并使用 `$VISUAL`、`$EDITOR` 或 `vi` 打开。

指定配置文件时：

```bash
glow --config ./project-glow.yml config init
glow --config ./project-glow.yml config show
glow --config ./project-glow.yml ./docs
```

#### `glow completion`

支持 `bash`、`elvish`、`fish`、`powershell` 和 `zsh`，输出 completion script 到标准输出。

## 配置文件与环境变量

配置优先级从高到低为：

1. CLI 参数
2. `GLOW_*` 环境变量
3. YAML 配置文件
4. 内置默认值

配置查找顺序：

1. `--config <FILE>`
2. `$GLOW_CONFIG_HOME/glow.yml`
3. `$XDG_CONFIG_HOME/glow/glow.yml`
4. 平台配置目录下的 `glow/glow.yml`

常见默认位置为 Linux 的 `~/.config/glow/glow.yml`，以及 macOS 的 `~/Library/Application Support/glow/glow.yml`。同目录下的 `.yaml` 扩展名也会被识别。

创建并查看配置：

```bash
glow config init
glow config path
glow config show
```

完整 YAML：

```yaml
style: auto
width: 0
pager: false
tui: false
all: false
showLineNumbers: false
preserveNewLines: false
mouse: false
```

为了兼容原 Glow 配置，`showLineNumbers` 和 `preserveNewLines` 保留 camelCase；`line_numbers` 和 `preserve_newlines` 也可以读取。

### 运行时环境变量

| 变量 | 说明 |
| --- | --- |
| `GLOW_CONFIG_HOME` | Glow 配置根目录 |
| `XDG_CONFIG_HOME` | XDG 配置根目录 |
| `GLOW_STYLE` | `auto`、`dark` 或 `light` |
| `GLOW_WIDTH` | 非负整数宽度 |
| `GLOW_PAGER` | 是否使用 pager |
| `GLOW_TUI` | 是否强制 TUI |
| `GLOW_ALL` | 是否包含隐藏/ignored 文件 |
| `GLOW_SHOWLINENUMBERS` | 是否显示代码行号 |
| `GLOW_PRESERVENEWLINES` | 是否保留 soft break |
| `GLOW_MOUSE` | 是否启用 TUI 鼠标 |
| `PAGER` | pager 命令，默认 `less -R` |
| `VISUAL`, `EDITOR` | `glow config edit` 使用的编辑器 |
| `NO_COLOR` | 存在时关闭单文档 ANSI 颜色 |
| `CLOUDFLARED_BIN` | `cloudflared` 可执行文件路径 |

布尔环境变量接受 `1/true/yes/on` 和 `0/false/no/off`，不区分大小写。

## Shell 补全

Bash：

```bash
mkdir -p "$HOME/.local/share/bash-completion/completions"
glow completion bash > "$HOME/.local/share/bash-completion/completions/glow"
```

Zsh（当前 shell）：

```zsh
source <(glow completion zsh)
```

Fish：

```fish
mkdir -p ~/.config/fish/completions
glow completion fish > ~/.config/fish/completions/glow.fish
```

## 安全模型与限制

Web 服务被设计为只读文档浏览器，而不是通用静态文件服务器：

- 只有扫描索引中的 Markdown 可以通过 `/doc/` 打开。
- `/asset/` 只包含明确 allowlist 的图片和 PDF。
- 不跟随符号链接；每次读取前再次 canonicalize 并确认路径仍位于文档根目录。
- URL 路由拒绝 traversal、反斜线、空路径段和跨平台分隔符绕过。
- Markdown 原始 HTML 被转义，最终 HTML 经过 sanitizer。
- MathML 使用独立的标签/属性 allowlist。
- Mermaid SVG 不直接插入 DOM，而是作为隔离的 data image 加载。
- 页面包含严格 CSP、禁止 framing、MIME sniffing、跨域 opener/resource 限制以及最小 permissions policy。
- 响应使用 `Cache-Control: no-store`，适合持续编辑预览。

需要注意：

- `glow serve --host 0.0.0.0` 会把没有鉴权的 HTTP 服务暴露到局域网或容器网络。
- `glow share` 会把整个选定文档根的允许内容暴露到匿名公网。
- `--all` 会显著扩大两者的扫描和暴露范围。
- HTTP(S) 远程文档属于不可信输入；Glow 会清洗内容，但仍应避免从未知来源下载敏感链接或附件。
- 该工具不是多用户 CMS，不提供编辑、账号、ACL、审计或持久化发布。

## 故障排查

| 现象 | 原因与处理 |
| --- | --- |
| `cargo` / `rustc` 找不到 | 重新运行 `bash scripts/build.sh`；脚本会安装 Rust。新 shell 可 `source "$HOME/.cargo/env"` |
| linker、`cc` 或 `xcrun` 错误 | macOS 安装 Command Line Tools；Linux 安装发行版的 C build toolchain |
| 安装完成但 `glow` 找不到 | 把安装目录（默认 `~/.local/bin`）加入 `PATH` |
| 目录浏览器提示需要 interactive terminal | TUI 不能写入管道；改用真实终端，或运行 `glow list DIR` |
| 没找到预期文档 | 用 `glow list DIR` 检查；如果文件隐藏或被忽略，再审慎尝试 `--all` |
| 终端没有颜色 | 检查输出是否被重定向，以及是否设置了 `NO_COLOR` |
| `less` 不存在 | 设置可用的 `$PAGER`，或不要使用 `--pager` |
| Linux `--open` 失败 | 安装/配置 `xdg-open`，或省略 `--open` 并复制打印的 URL |
| `cloudflared was not found` | 安装 cloudflared、修复 `PATH`，或传 `--cloudflared` / `CLOUDFLARED_BIN` |
| 等待 Quick Tunnel URL 超时 | 检查网络、防火墙、WARP 和 `~/.cloudflared/config.yaml`；增加 `--timeout` 只会延长 URL 等待时间 |
| `cloudflared` 在 URL 前退出 | Glow 会显示退出状态和最近输出；需要完整日志时使用下面的手动调试方式 |
| URL 已打印但短暂不可达 | 等待 DNS/边缘配置生效，并确认 Glow 与 cloudflared 仍在运行 |
| Mermaid 显示错误 | 展开 `Mermaid source`，检查图类型和语法；注意纯 Rust renderer 与官方 JS 的差异 |
| 公式没有排版 | 检查 `$` / `$$` 是否配对以及命令是否受 renderer 支持；失败时保留的源码会帮助定位 |

需要查看完整 `cloudflared` 日志时，可将两个进程分开运行：

```bash
# 终端 A
glow serve ./docs --port 8080

# 终端 B
cloudflared tunnel --no-autoupdate --url http://127.0.0.1:8080
```

不要直接使用无范围的 `pkill cloudflared`，它可能终止机器上其他 Tunnel。需要清理异常遗留进程时，先用 `ps` 确认具体 PID。

## 开发与测试

常用命令：

```bash
cargo fmt --all -- --check
cargo clippy --all-targets --all-features --locked -- -D warnings
cargo test --all-targets --locked
cargo build --release --locked
cargo run -- .
```

构建脚本自身可以这样检查：

```bash
bash -n scripts/build.sh
bash scripts/build.sh --help
bash scripts/build.sh --no-install
```

也可以使用 Docker 构建：

```bash
docker build -t glow-rust .

# 本地 Web 文档站
docker run --rm -p 8080:8080 \
  -v "$PWD:/docs:ro" \
  glow-rust serve /docs --host 0.0.0.0 --port 8080
```

### 代码结构

| 模块 | 责任 |
| --- | --- |
| `src/cli.rs` | Clap 命令和参数定义 |
| `src/main.rs` | 配置合并、命令分发、终端输出 |
| `src/discover.rs` | 递归目录扫描和索引 |
| `src/document.rs` | Markdown 类型、标题和 frontmatter |
| `src/render/terminal.rs` | ANSI/TUI Markdown 渲染 |
| `src/render/html.rs` | HTML、MathML、Mermaid 和链接改写 |
| `src/tui.rs` | 交互式目录树和预览 |
| `src/web.rs` | 只读 Web server、路由、安全 header 和自动刷新 |
| `src/tunnel.rs` | `cloudflared` 子进程、URL 验证与关闭流程 |
| `src/source.rs` | 文件、stdin、URL、GitHub/GitLab 来源 |
| `src/config.rs` | YAML、环境变量和默认配置路径 |
| `assets/` | 编译进二进制的 Web CSS/JavaScript |
| `tests/cli.rs` | CLI 集成测试 |

项目使用 `unsafe_code = "forbid"`，release profile 开启 thin LTO、单 codegen unit，并移除符号。CI 在 macOS、Linux 和 Windows 上执行 Rust 测试；本仓库的一键 Bash 构建脚本专门面向 macOS/Linux。

本项目的目录导航与阅读体验参考了 [aydiler/md-viewer](https://github.com/aydiler/md-viewer)，并针对终端优先、本地 Web 阅读和单二进制分发进行了重新设计。

## License

MIT，详见 [`LICENSE`](LICENSE)。版权归原 Glow contributors 所有。
