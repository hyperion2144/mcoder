# mcoder 三端 UI 设计规范 v2

> **设计哲学**：简洁 · 精致 · 不喧嚣
> 基于 Tokyo Night 配色，为 agent orchestration 工具精炼而成。
> 中暗背景（非纯黑）、低饱和度强调色、JetBrains Mono 代码字体、Inter UI 字体。

本规范对 **TUI / Desktop / Mobile** 三端统一起效。所有 UI 改动必须遵守本规范。

---

## 目录

1. [设计原则](#1-设计原则)
2. [颜色 Token](#2-颜色-token)
3. [角色色（卡片分类）](#3-角色色卡片分类)
4. [边框风格](#4-边框风格)
5. [字号 / 间距节奏](#5-字号--间距节奏)
6. [标题规范](#6-标题规范)
7. [Loading 流动光效](#7-loading-流动光效)
8. [排版规则](#8-排版规则)
9. [TUI 布局规范](#9-tui-布局规范)
10. [三端对齐表](#10-三端对齐表)
11. [移除 AI 味清单](#11-移除-ai-味清单)
12. [违规检查清单](#12-违规检查清单)

---

## 1. 设计原则

### 1.1 三原则

| 原则 | 含义 |
|------|------|
| **简洁** | 每屏信息密度 ≤ 一屏可读完；不堆叠装饰 |
| **精致** | 细节有打磨（对齐、节奏、token 一致）|
| **不喧嚣** | 动画只在必要场景出现；颜色克制；不用 emoji 卖萌 |

### 1.2 禁止事项

- 任何 emoji 装饰字符（🔒 ✓ ⚠ 💡 🚀 ⠋ 等；`✓` 和 `✗` 是允许的语义字符）
- `--` / `───` / `===` ASCII 分隔符
- `italic` 修饰（TUI 终端多数不可见）
- `dimColor` 滥用
- inline style 写 hex 颜色（必须用 token）
- 不在 8 倍数节奏上的 spacing

---

## 2. 颜色 Token

基于 **Tokyo Night** 调色板。

### 2.1 原始 Token 刻度

| 用途 | Token | TUI name | CSS hex | 说明 |
|------|-------|----------|---------|------|
| **背景** | `--mc-background` | - | `#1a1b26` | 页面底色 |
| **卡片** | `--mc-card` | - | `#16161e` | 卡片表面 |
| **浮起** | `--mc-popover` | - | `#292e42` | 浮起元素 |
| **边框** | `--mc-border` | - | `#292e42` | 边框色 |
| **文字-主** | `--mc-foreground` | `white` | `#c0caf5` | 主要内容文字 |
| **文字-次** | `--mc-neutral-700` | `gray` | `#9aa5ce` | 辅助说明 |
| **文字-弱** | `--mc-muted-foreground` | `gray` | `#565f89` | 占位、提示 |
| **品牌/主色** | `--mc-primary` | `blue` | `#7aa2f7` | 主品牌色 / 执行类 |
| **成功** | `--mc-state-success` | `green` | `#9ece6a` | 成功状态 |
| **警告** | `--mc-state-warning` | `yellow` | `#e0af68` | 待操作 / 警告 |
| **错误** | `--mc-state-error` | `red` | `#f7768e` | 错误 / 失败 |
| **紫罗兰** | `--mc-state-mauve` | `magenta` | `#bb9af7` | 思考 / 推理类 |
| **青色** | `--mc-state-info` | `cyan` | `#7dcfff` | 信息 / 路径 |
| **橙色** | `--mc-state-orange` | `yellow` | `#ff9e64` | 橙色（TUI 映射到 yellow） |

### 2.2 TUI 颜色使用规则

TUI 没有背景色（继承终端），ink 支持的色名映射到 Tokyo Night：

```typescript
// mcoder-tui/src/theme.ts
export const TUI_COLORS = {
  brand: 'blue',         // #7aa2f7
  accent: 'blue',         // #7aa2f7 (alias)
  textPrimary: 'white',   // #c0caf5
  textSecondary: 'gray',  // #a9b1d6
  textMuted: 'gray',      // #565f89
  success: 'green',       // #9ece6a
  warning: 'yellow',      // #e0af68
  error: 'red',           // #f7768e
  mauve: 'magenta',       // #bb9af7
  cyan: 'cyan',           // #7dcfff
  orange: 'yellow',       // #ff9e64
} as const;
```

### 2.3 Desktop / Mobile CSS 变量

```css
:root {
  --mc-background: #1a1b26;
  --mc-foreground: #c0caf5;
  --mc-card: #16161e;
  --mc-popover: #292e42;
  --mc-primary: #7aa2f7;
  --mc-secondary: #292e42;
  --mc-muted: #1f2335;
  --mc-muted-foreground: #565f89;
  --mc-accent: #7aa2f7;
  --mc-destructive: #f7768e;
  --mc-border: #292e42;
  --mc-input: #292e42;
  --mc-ring: #7aa2f7;
  --mc-radius-sm: 4px;
  --mc-radius-md: 8px;
  --mc-radius-lg: 12px;
  --mc-font-sans: 'Inter', -apple-system, sans-serif;
  --mc-font-mono: 'JetBrains Mono', 'SF Mono', monospace;
}
```

---

## 3. 角色色（卡片分类）

所有卡片**只用一种边框色**，按用途分 5 类：

| 类别 | Token | TUI name | 用途 |
|------|-------|----------|------|
| **interaction** | warning | `yellow` | 待用户操作（ask_user / permission / plan） |
| **execution** | accent | `blue` | agent 主动执行（write / edit / bash / read / search） |
| **thinking** | mauve | `magenta` | agent 推理 / 思考 |
| **done** | text-muted | `gray` | 已完成（折叠默认） |
| **error** | error | `red` | 失败 |

---

## 4. 边框风格

| 场景 | TUI | Desktop / Mobile |
|------|-----|------------------|
| **交互卡片**（ask / permission / plan） | `borderStyle="round"` + warning | `border: 1px solid var(--mc-state-warning)` |
| **执行卡片**（tool calls） | `borderStyle="round"` + accent | `border: 1px solid var(--mc-primary)` |
| **思考卡片**（thinking） | `borderStyle="round"` + mauve | `border: 1px solid var(--mc-state-mauve)` |
| **面板**（session / todo / setting） | `borderStyle="single"` + text-muted | `border: 1px solid var(--mc-border)` |
| **底部 dock**（input + todos） | `borderStyle="round"` + text-muted | `border: 1px solid var(--mc-border)` |

---

## 5. 字号 / 间距节奏

### 5.1 间距（8pt grid）

| 用途 | 值 |
|------|-----|
| 卡片内边距 | `12px` |
| 卡片外边距 | `8px` |
| 卡片内元素 gap | `8px` |
| 段落间距 | `16px` |
| 大区块间距 | `24px` |

### 5.2 圆角

| 用途 | Desktop | Mobile |
|------|---------|--------|
| 按钮 | `4px` | `6px` |
| 卡片 | `8px` | `12px` |
| Input | `4px` | `6px` |

### 5.3 字号

| 用途 | Desktop | Mobile |
|------|---------|--------|
| 卡片标题 | `14px` bold | `15px` bold |
| 正文 | `14px` | `15px` |
| 弱文字 | `13px` | `14px` |
| 代码 / mono | `14px` | `15px` |

> TUI 不控制字号，仅靠颜色区分主次。字体为 `JetBrains Mono`（Desktop/Mobile CSS）/ 终端默认（TUI）。

---

## 6. 标题规范

### 6.1 通用格式

```
<前缀符号> <类别> · <子状态>
```

- **前缀符号**（5 种语义化字符）：
  - `▸` 待操作 / 折叠
  - `▶` 执行中
  - `✓` 已完成
  - `✗` 失败
  - `?` 待审批

### 6.2 标题示例

| 场景 | 标题 |
|------|------|
| 待审批 ask_user | `▸ ask_user · waiting for input` |
| 执行中 write | `▶ write foo.rs` |
| 已完成 write | `✓ write foo.rs` |
| 失败 bash | `✗ bash npm test` |
| 思考中 | `▶ thinking` |

---

## 7. Loading 流动光效

### 7.1 何时启用

所有正在执行的工具卡片和 thinking 卡片必须启用流光。

### 7.2 视觉规范

逐字符扫描：每个字符的亮度按 sin 波在 0.35 ~ 1.0 之间循环。

**实现参数**：
- 帧间隔：80ms（12.5fps）
- 波形：`sin(i / N * π * 2 + phase)`，`phase` 随时间累加 `+0.4`
- 亮度区间：0.35 ~ 1.0
- 颜色：TUI `white` -> `gray` 三档；Desktop/Mobile `var(--mc-foreground)` -> `var(--mc-muted-foreground)`

### 7.3 TUI 实现

```typescript
// ShimmerText.tsx - 三档亮度模拟
if (brightness < 0.5) return <Text color="gray">{ch}</Text>;
if (brightness < 0.85) return <Text color="white">{ch}</Text>;
return <Text color="white" bold>{ch}</Text>;
```

---

## 8. 排版规则

### 8.1 分隔符

| 场景 | 分隔符 |
|------|--------|
| 字段并列 | ` · `（中点 + 空格）|
| 区块 | 顶部 1px border-bottom + 小字标题 |
| 多项 | 换行 + 缩进 |

### 8.2 缩进

| 元素 | TUI | Desktop/Mobile |
|------|-----|----------------|
| 卡片内主内容 | `paddingX=1` | `12px` |
| 卡片内子项 | `marginLeft=2` | `4px` |
| 代码块 | `marginLeft=4` | `12px` |

---

## 9. TUI 布局规范

### 9.1 主界面布局（tui-main）

```
┌─ header-card ──────────────────────────────────────┐
│ mcoder v0.1.0                                      │
│ Tips: # prompt  / commands  ! bash  $ python       │
│ LSP Servers: ● typescript  ● rust                  │
│ Recent sessions: ● Refactor login  ● Fix auth bug  │
└────────────────────────────────────────────────────┘
┌─ messages stream ──────────────────────────────────┐
│ user                                               │
│   Refactor login.ts to use async/await             │
│ ▶ thinking                                         │
│ │ ▶ read src/login.ts                              │
│ │   10 lines shown, 152 total                      │
│ │ ✓ edit_replace src/login.ts                      │
│ mcoder                                             │
│   Login has been refactored...                     │
└────────────────────────────────────────────────────┘
┌─ bottom dock ──────────────────────────────────────┐
│ todos (3)                                          │
│ ● Convert login() to async                         │
│ ○ Update calling sites                             │
│ ┌─ input-box ────────────────────────────────────┐ │
│ │ claude-sonnet-4 · default · ~/projects/mcoder  │ │
│ │ 12.4k / 200k (6.2%) · $0.03 · streaming        │ │
│ │ > _                                              │ │
│ │ [Enter] send  [Shift+Enter] newline  [/] cmd   │ │
│ └──────────────────────────────────────────────────┘ │
└────────────────────────────────────────────────────┘
```

### 9.2 组件层级

```
App (flexDirection=column, height=100%)
├── MessageList (flexGrow=1, overflow=hidden)
│   ├── HeaderCard (无消息时显示)
│   ├── MessageView[] (消息流)
│   └── ShimmerText (streaming 时)
├── PlanApproval
├── 覆盖层视图 (sessions/todos/tasks/config/help/...)
├── TodoSummaryBar (inline section)
├── SubagentBar (inline section)
├── ResumeBar
├── CommandPicker (输入 / 时)
└── InputBox (border=round, 固定底部)
    ├── session-info (model · role · path · branch)
    ├── usage (context · cost · streaming)
    ├── TextInput (> prompt)
    └── hints ([Enter] send [/] cmd [@] files)
```

### 9.3 Header Card（欢迎屏）

无消息时显示，有消息后隐藏：

- `mcoder` (bold cyan) + `v{version}` (muted)
- `Tips` (bold) + `# prompt  / commands  ! bash  $ python` (muted)
- `LSP Servers` (bold) + `● {lang}` 列表 (muted)
- `Recent sessions` (bold) + `● {title}` 列表 (muted)

### 9.4 Input Box（底部 dock）

- 边框：`round` + `textMuted`
- 第一行：`{model}` (blue) ` · ` `{role}` (mauve if non-default) ` · ` `{path}` (cyan) ` · ` `{branch}` (muted)
- 第二行：`{context_used} / {context_window}` (muted) ` · ` `{cost}` (muted) ` · ` `streaming` (green if running)
- 第三行：`> ` (blue) + TextInput
- 第四行：`[Enter] send  [Shift+Enter] newline  [/] commands  [@] files` (muted)

---

## 10. 三端对齐表

| 元素 | TUI | Desktop | Mobile |
|------|-----|---------|--------|
| **卡片边框** | `round` / `single` + 角色色 | `solid 1px` + 角色色 | `solid 1px` + 角色色 |
| **卡片内边距** | `paddingX=1` | `12px` | `12px` |
| **卡片圆角** | (无) | `8px` | `12px` |
| **标题字号** | (固定，靠 bold) | `14px` bold | `15px` bold |
| **正文字号** | (固定) | `14px` | `15px` |
| **代码字号** | (固定) | `14px` mono | `15px` mono |
| **loading 光效** | ShimmerText（三档亮度） | CSS gradient + animation | CSS gradient + animation |
| **字体** | 终端默认 | Inter (UI) / JetBrains Mono (code) | 同 Desktop |

---

## 11. 移除 AI 味清单

### 11.1 文案替换表

| 旧（AI 味） | 新（简洁） |
|-------------|----------|
| `ask_user (等待你的回答)` | `▸ ask_user · waiting for input` |
| `── Input ──` | 小字 `Input` + 顶部分割线 |
| `running...` | 删除（标题 `▶ name` 自带流光）|
| `thinking...` | `▶ thinking` 流光 |
| `Press any key to continue` | 删除 |
| `✅ Done!` | `✓ done` |
| `🔒 Permission Required` | `▸ permission · waiting for approval` |

### 11.2 文案原则

1. **动词开头**：标题用工具名（`write` / `bash` / `ask_user`）
2. **状态在右**：状态描述在 `·` 之后
3. **不解释**：删除"提示用户该怎么做"的文案
4. **中英一致**：中文文案与英文文案句式对称

---

## 12. 违规检查清单

### 颜色
- [ ] 没有硬编码 hex（必须用 token）
- [ ] 没有 inline style 写颜色
- [ ] 卡片边框色符合 §3 角色色分类
- [ ] 没有 6+ 种颜色滥用

### 排版
- [ ] 标题格式符合 §6（前缀符号 + 类别 · 状态）
- [ ] spacing 在 8 倍数节奏上
- [ ] 没有 emoji 装饰符
- [ ] 没有 ASCII 分隔符
- [ ] 没有 `italic` 修饰
- [ ] 没有 `press ESC to close` 这类提示

### Loading
- [ ] 写类工具（write/edit/ast_edit）有流光
- [ ] 执行类工具（bash/launch）有流光
- [ ] 读类工具（read/grep/glob）有流光
- [ ] thinking 卡片有流光
- [ ] 流光在完成后立即停止

### 一致性
- [ ] 三端使用相同的 token 名
- [ ] TUI 用 `TUI_COLORS` 导出色名
- [ ] Desktop/Mobile 用 `var(--mc-*)` 变量
- [ ] 按钮尺寸符合 §10 对齐表

---

## 附录 A：TUI 颜色对照速查

| Token | TUI 色名 | CSS hex |
|-------|----------|---------|
| accent/primary | `blue` | `#7aa2f7` |
| success | `green` | `#9ece6a` |
| warning | `yellow` | `#e0af68` |
| error | `red` | `#f7768e` |
| mauve | `magenta` | `#bb9af7` |
| cyan/info | `cyan` | `#7dcfff` |
| text-primary | `white` | `#c0caf5` |
| text-muted | `gray` | `#565f89` |

## 附录 B：状态前缀字符

```
▸  待操作
▶  执行中
✓  已完成
✗  失败
?  待审批
●  状态点（活跃）
○  状态点（待办）
│  树枝线
▾  展开折叠
·  分隔符
```

## 附录 C：变更历史

| 日期 | 改动 |
|------|------|
| 2026-07-31 | 初版（Catppuccin Mocha） |
| 2026-08-03 | v2：迁移到 Tokyo Night 配色；新增 TUI 布局规范（header card + bottom dock）；更新 InputBox / ToolCard / MessageList / TodoSummaryBar |
