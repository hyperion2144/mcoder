# mcoder 三端 UI 设计规范

> **设计哲学**：简洁 · 精致 · 不喧嚣
> 不堆砌 emoji · 不滥用颜色 · 不重复分隔符 · 不留 AI 味

本规范对 **TUI / Desktop / Mobile** 三端统一起效。所有 UI 改动必须遵守本规范，PR review 时按本规范逐条对照。

---

## 目录

1. [设计原则](#1-设计原则)
2. [颜色 Token](#2-颜色-token)
3. [角色色（卡片分类）](#3-角色色卡片分类)
4. [边框风格](#4-边框风格)
5. [字号 / 间距节奏](#5-字号--间距节奏)
6. [标题规范](#6-标题规范)
7. [Loading 流动光效（核心）](#7-loading-流动光效核心)
8. [排版规则](#8-排版规则)
9. [三端对齐表](#9-三端对齐表)
10. [移除 AI 味清单](#10-移除-ai-味清单)
11. [实施示例](#11-实施示例)
12. [违规检查清单](#12-违规检查清单)

---

## 1. 设计原则

### 1.1 三原则

| 原则 | 含义 |
|------|------|
| **简洁** | 每屏信息密度 ≤ 一屏可读完；不堆叠装饰 |
| **精致** | 细节有打磨（对齐、节奏、token 一致）|
| **不喧嚣** | 动画只在必要场景出现；颜色克制；不用 emoji 卖萌 |

### 1.2 禁止事项（违反即不通过 review）

- ❌ 任何 emoji 装饰字符（🔒 ✓ ⚠ 💡 🚀 ⠋ 等）
- ❌ `--` / `───` / `===` ASCII 分隔符
- ❌ `italic` 修饰（TUI 终端多数不可见，等同废弃）
- ❌ `dimColor` 滥用（视觉噪音）
- ❌ 中英混合装饰文本（如 `▸ ask_user (等待你的回答)`）
- ❌ 工具卡片按 sub-category 用 6+ 种颜色
- ❌ inline style 写 hex 颜色（必须用 token）
- ❌ 不在 8 倍数节奏上的 spacing（10/14/18 等）

---

## 2. 颜色 Token

所有颜色来自 catppuccin mocha 调色板，**严禁在代码里硬编码 hex**（TUI 用色名，Desktop/Mobile 用 CSS 变量）。

| 用途 | Token | TUI name | CSS hex | 用途说明 |
|------|-------|----------|---------|----------|
| **背景** | `--bg-base` | — | `#1e1e2e` | 页面底色 |
| **表面** | `--bg-surface` | — | `#181825` | 卡片表面 |
| **浮起** | `--bg-elevated` | — | `#313244` | 浮起的元素（badge bg、input） |
| **悬停** | `--bg-hover` | — | `#45475a` | 鼠标悬停态 |
| **激活** | `--bg-active` | — | `#585b70` | 按钮按下态 |
| **弱化** | `--bg-overlay` | — | `#6c7086` | 弱背景（spinner 弱端） |
| **文字-主** | `--text-primary` | `white` | `#cdd6f4` | 主要内容文字 |
| **文字-次** | `--text-secondary` | `gray` | `#a6adc8` | 辅助说明 |
| **文字-弱** | `--text-muted` | `gray` | `#6c7086` | 占位、提示 |
| **accent** | `--accent` | `cyan` | `#89b4fa` | 主品牌色 / 执行类 |
| **success** | `--success` | `green` | `#a6e3a1` | 成功状态 |
| **warning** | `--warning` | `yellow` | `#f9e2af` | 待操作 / 警告 |
| **error** | `--error` | `red` | `#f38ba8` | 错误 / 失败 |
| **mauve** | `--mauve` | `magenta` | `#cba6f7` | 思考 / 推理类 |

### 2.1 TUI 颜色使用规则

TUI 没有背景色（继承终端），ink 支持的色名映射到 catppuccin：

```typescript
// mcoder-tui/src/theme.ts（统一导出）
export const TUI_COLORS = {
  bgBase: null,             // 透明
  bgElevated: null,         // 透明
  textPrimary: 'white',
  textSecondary: 'gray',
  textMuted: 'gray',
  accent: 'cyan',
  success: 'green',
  warning: 'yellow',
  error: 'red',
  mauve: 'magenta',
} as const;
```

### 2.2 Desktop / Mobile CSS 变量定义

```css
/* mcoder-desktop/src/styles.css 与 mcoder-mobile/src/styles.css 同源 */
:root {
  --bg-base: #1e1e2e;
  --bg-surface: #181825;
  --bg-elevated: #313244;
  --bg-hover: #45475a;
  --bg-active: #585b70;
  --bg-overlay: #6c7086;
  --text-primary: #cdd6f4;
  --text-secondary: #a6adc8;
  --text-muted: #6c7086;
  --accent: #89b4fa;
  --accent-dim: rgba(137, 180, 250, 0.12);
  --success: #a6e3a1;
  --success-dim: rgba(166, 227, 161, 0.12);
  --warning: #f9e2af;
  --warning-dim: rgba(249, 226, 175, 0.12);
  --error: #f38ba8;
  --error-dim: rgba(243, 139, 168, 0.12);
  --mauve: #cba6f7;
  --mauve-dim: rgba(203, 166, 247, 0.12);
  --border: #313244;
  --border-subtle: #45475a;
  --radius-sm: 4px;
  --radius-md: 6px;
  --radius-lg: 8px;
  --transition: 0.15s ease;
  --font-mono: 'SF Mono', 'Menlo', 'Monaco', 'Cascadia Code', monospace;
}
```

---

## 3. 角色色（卡片分类）

所有卡片**只用一种边框色**，按用途分 5 类：

| 类别 | Token | TUI name | 用途 |
|------|-------|----------|------|
| **interaction** | warning | `yellow` | 待用户操作（ask_user / permission / plan approval） |
| **execution** | accent | `cyan` | agent 主动执行（write / edit / bash / read / search） |
| **thinking** | mauve | `magenta` | agent 推理 / 思考 |
| **done** | text-muted | `gray` | 已完成（折叠默认） |
| **error** | error | `red` | 失败 |

**反例**（旧 ToolCard）：thinking/file/command/code/graph/subagent/plan/workflow/other 9 种分类 9 种颜色。

**正例**（新 ToolCard）：所有工具归一为 `execution` 一种颜色。

---

## 4. 边框风格

| 场景 | TUI | Desktop / Mobile |
|------|-----|------------------|
| **交互卡片**（ask / permission / plan） | `borderStyle="round"` + warning | `border: 1px solid var(--warning)` + `background: var(--warning-dim)` |
| **执行卡片**（tool calls） | `borderStyle="round"` + accent | `border: 1px solid var(--accent)` + `background: var(--bg-surface)` |
| **思考卡片**（thinking） | `borderStyle="round"` + mauve | `border: 1px solid var(--mauve)` + `background: var(--mauve-dim)` |
| **面板**（session list / todo / tree / setting / config / help） | `borderStyle="single"` + text-muted | `border: 1px solid var(--border-subtle)` + `background: var(--bg-surface)` |
| **摘要**（已回答 / 已完成 / 已决议） | `borderStyle="single"` + text-muted | `border: 1px solid var(--border-subtle)` + `background: var(--bg-surface)` + `opacity: 0.85` |
| **status bar**（顶部 / 底部固定） | `borderStyle="single"` + text-muted | `border-bottom/top: 1px solid var(--border-subtle)` |
| **消息流角色行** | 无 border，靠左侧 `│` 引导 | 无 border，靠左侧 2px 实心引导条 |

> **原则**：border 是结构的体现，不是装饰。

---

## 5. 字号 / 间距节奏

### 5.1 间距（8 倍数）

```
可用值：4 · 8 · 12 · 16 · 24 · 32
禁止值：10 · 14 · 18 · 20
```

| 用途 | 值 |
|------|-----|
| 卡片内边距 | `12px` |
| 卡片外边距 | `8px` |
| 卡片内元素 gap | `8px` |
| 卡片内行高 | `4px` |
| 段落间距 | `16px` |
| 大区块间距 | `24px` |

### 5.2 圆角

| 用途 | 值 |
|------|-----|
| 按钮 | `4px` (Desktop) / `6px` (Mobile) |
| 卡片 | `6px` (Desktop) / `8px` (Mobile) |
| Badge | `4px` |
| Input | `4px` (Desktop) / `6px` (Mobile) |

### 5.3 字号（仅 Desktop / Mobile）

| 用途 | Desktop | Mobile |
|------|---------|--------|
| 卡片标题 | `13px` bold | `14px` bold |
| 正文 | `12px` | `13px` |
| 弱文字 / 提示 | `11px` | `12px` |
| 代码 / mono | `12px` | `13px` |
| 按钮 | `12px` | `15px` (大触摸区) |

> TUI 不控制字号，仅靠颜色区分主次。

---

## 6. 标题规范

### 6.1 通用格式

```
<前缀符号> <类别> · <子状态> · <badge>
```

- **前缀符号**（单字符，语义化，**仅 5 种**）：
  - `▸` 待操作 / 折叠
  - `▶` 执行中
  - `✓` 已完成
  - `✗` 失败
  - `?` 待审批

- **类别**（卡片类型）：
  - `ask_user` / `permission` / `plan` / `write` / `bash` / `thinking` 等

- **子状态**（可选）：
  - `等待输入` / `等待确认` / `已通过` / `已拒绝`

- **badge**（方括号）：
  - `[STD]` / `[YOLO]` / `[STRICT]`（权限级别）
  - `[3/5]`（进度）
  - `[error]`（异常）

### 6.2 标题示例

| 场景 | 标题 |
|------|------|
| 待审批 ask_user | `▸ ask_user · 等待输入` |
| 待审批 permission | `▸ permission · STD · 等待确认` |
| 执行中 write | `▶ write foo.rs` |
| 执行中 bash | `▶ bash npm test` |
| 已完成 write | `✓ write foo.rs` |
| 失败 bash | `✗ bash npm test` |
| 思考中 | `▶ Thinking` |
| 已回答 ask | `ask_user · 已回答` |

### 6.3 字号 / 颜色

| 端 | 标题样式 |
|----|----------|
| TUI | `bold` + 角色色（warning / accent / mauve / gray / red） |
| Desktop | `13px` `font-weight: 600` + 角色色 |
| Mobile | `14px` `font-weight: 600` + 角色色 |

---

## 7. Loading 流动光效（核心）

### 7.1 何时启用

**所有正在执行的工具卡片和思考卡片必须启用**：

| 必须有 | 工具 / 场景 |
|--------|------------|
| ✅ | `write` / `edit` / `ast_edit` |
| ✅ | `bash` / `launch` |
| ✅ | `read` / `grep` / `glob` / `code_graph_*` |
| ✅ | `lsp_*` / `ast_query` |
| ✅ | `mcp_*` / `browser_*` / `screen_*` / `app_*` |
| ✅ | **thinking 卡片**（LLM 流式推理）|
| ❌ | 已完成（✓）/ 失败（✗）/ 待用户操作（▸） |

### 7.2 视觉规范

逐字符扫描：每个字符的亮度按 sin 波在 0.35 ~ 1.0 之间循环，整体向前推进。

```
字符位置:  0    1    2    3    4    5    6    7
亮度:      ░░░▒▒▓▓██▓▓▒▒░░░░░░▒▒▓▓██▓▓▒▒░░
           ←─── 高亮波峰 ───→
           ←──── 低亮度尾部 ────→
```

**实现参数**：

- 帧间隔：80ms（12.5fps）
- 波形：`sin(i / N * π * 2 + phase)`，`phase` 随时间累加 `+0.4`
- 亮度区间：0.35 ~ 1.0
- 颜色：TUI `white` → `bg-overlay` 渐变；Desktop/Mobile `var(--text-primary)` → `var(--bg-overlay)`

### 7.3 TUI 实现

```typescript
// mcoder-tui/src/components/ShimmerText.tsx
import React, { useState, useEffect } from 'react';
import { Text } from 'ink';

export function ShimmerText({ text }: { text: string }) {
  const [phase, setPhase] = useState(0);
  useEffect(() => {
    const id = setInterval(() => setPhase(p => p + 0.4), 80);
    return () => clearInterval(id);
  }, []);

  if (!text) return null;
  return (
    <Text>
      {text.split('').map((ch, i) => {
        const wave = Math.sin((i / Math.max(text.length, 1)) * Math.PI * 2 + phase);
        const brightness = 0.35 + 0.65 * Math.max(0, wave);
        // 三档亮度：gray（暗）/ white（亮）/ white bold（最亮）
        if (brightness < 0.5) return <Text key={i} color="gray">{ch}</Text>;
        if (brightness < 0.85) return <Text key={i} color="white">{ch}</Text>;
        return <Text key={i} color="white" bold>{ch}</Text>;
      })}
    </Text>
  );
}
```

> TUI 不支持真彩色 RGB，所以用三档亮度（gray / white / white bold）模拟。

### 7.4 Desktop / Mobile 实现（CSS 动画）

```css
/* 流动光效 - 适用 .tool-card[data-loading="true"] .tool-card-title */
@keyframes shimmer {
  0% { background-position: -200% 0; }
  100% { background-position: 200% 0; }
}

.tool-card[data-loading="true"] .tool-card-title {
  background: linear-gradient(
    90deg,
    var(--bg-overlay) 0%,
    var(--bg-overlay) 30%,
    var(--text-primary) 50%,
    var(--bg-overlay) 70%,
    var(--bg-overlay) 100%
  );
  background-size: 200% 100%;
  background-clip: text;
  -webkit-background-clip: text;
  color: transparent;
  -webkit-text-fill-color: transparent;
  animation: shimmer 1.6s ease-in-out infinite;
}
```

### 7.5 thinking 卡片特殊性

LLM 推理流式输出，**持续时间最长**。处理方式：

1. **首行**：标题 `▶ Thinking` + 流光
2. **正文**：折叠的 markdown 文本块，随流式追加自动展开
3. **结束**：标题变 `✓ done` 或 `✗ failed`（取决于是否有工具调用），流光停止

---

## 8. 排版规则

### 8.1 分隔符统一

**所有卡片内部仅用以下分隔符**：

| 场景 | 分隔符 |
|------|--------|
| 字段并列（标题 + 状态 + badge） | ` · `（中点 + 空格）|
| 区块（Input / Result） | 顶部 1px border-bottom + 小字标题 |
| 多项（todo / plan steps） | 换行 + 缩进 |

**禁止**：

- ❌ `── Input ──`
- ❌ `---`
- ❌ `***`
- ❌ `===`

### 8.2 缩进规则

| 元素 | 缩进 |
|------|------|
| 卡片内主内容 | `0`（顶满）|
| 卡片内子项 | `4px` / TUI `marginLeft=2` |
| 嵌套层 | 上一级 + `8px` |
| 代码块 | `12px` |

### 8.3 字体

- 默认：系统 sans-serif
- 代码：monospace token (`--font-mono`)
- **禁止**：自定义特殊字体（除 mono）

---

## 9. 三端对齐表

| 元素 | TUI | Desktop | Mobile |
|------|-----|---------|--------|
| **卡片边框** | `round` / `single` + 角色色 | `solid 1px` + 角色色 | `solid 1px` + 角色色 |
| **卡片内边距** | `paddingX=1` | `12px` | `12px` |
| **卡片外边距** | `marginY=1` | `8px 0` | `8px 0` |
| **卡片圆角** | (无) | `6px` | `8px` |
| **标题字号** | (固定，靠 bold) | `13px` `font-weight: 600` | `14px` `font-weight: 600` |
| **正文字号** | (固定) | `12px` | `13px` |
| **弱文字** | `gray` | `--text-muted` `11px` | `--text-muted` `12px` |
| **代码字号** | (固定) | `12px` mono | `13px` mono |
| **按钮 padding** | (无按钮) | `6px 12px` | `12px 16px` |
| **按钮圆角** | — | `4px` | `6px` |
| **按钮字号** | — | `12px` `font-weight: 600` | `15px` `font-weight: 600` |
| **loading 光效** | ShimmerText（三档亮度） | CSS gradient + animation | CSS gradient + animation |
| **loading 帧率** | 80ms（12.5fps）| 1.6s 一周期 | 1.6s 一周期 |

---

## 10. 移除 AI 味清单

### 10.1 文案替换表

| 旧（AI 味） | 新（简洁） |
|-------------|----------|
| `ask_user (等待你的回答)` | `▸ ask_user · 等待输入` |
| `permission (等待确认)` | `▸ permission · STD · 等待确认` |
| `── Input ──` | 小字 `Input` + 顶部 1px 分割线 |
| `── Result ──` | 小字 `Result` + 顶部 1px 分割线 |
| `running...` | 删除（标题 `▶ name` 自带流光）|
| `thinking...` | `▶ Thinking` 流光 + 实时 markdown |
| `press ESC to close` | 删除（status bar 已有快捷键提示）|
| `lsp: typescript, rust` | 删除或合并 |
| `model: gpt-4o  project: ~/foo` | 合并为 `gpt-4o · ~/foo` |
| `(多选，可选多个)` | `multi-select` |
| `↑ 当前问题（直接输入文字作为 note）` | focus 用 `▶` 标识 |
| `+ 5 more` | `+5 more` |
| `▸ ask_user (已回答)` | `ask_user · 已回答` |
| `Press any key to continue` | 删除 |
| `✅ Done!` | `✓ done` |
| `⚠ Warning` | `! warning` |
| `🔒 Permission Required` | `▸ permission · 等待确认` |
| `💡 Tip:` | 删除（不解释）|

### 10.2 视觉移除

- ❌ emoji 装饰（🔒 ✓ ⚠ 💡 🚀 ⠋ 等）—— 注意 `✓` 和 `✗` 是允许的（语义字符，非 emoji）
- ❌ 彩虹色工具卡片（6+ 色 → 1 色）
- ❌ ASCII 分隔符（`---` `===` `───`）
- ❌ `Press any key to continue`
- ❌ 装饰性图标（loading 时除 `▶` 外的其他字符）

### 10.3 文案原则

1. **动词开头**：标题用动词或工具名（`write` / `bash` / `ask_user`），不用名词短语
2. **状态在右**：状态描述在 `·` 之后，不放最前
3. **不解释**：删除"提示用户该怎么做"的文案（用户已会）
4. **不重复**：相同信息不出现两次
5. **中英一致**：中文文案与英文文案句式对称

---

## 11. 实施示例

### 11.1 AskUserCard（TUI）改造

```diff
- <Box flexStyle="round" borderColor="yellow">
-   <Text color="yellow" bold>▸ ask_user (等待你的回答)</Text>
-   <Text color="cyan">↑ 当前问题（直接输入文字作为 note）</Text>
-   <Text color="gray">输入 1-4 选择 · 文字作为 note · Enter 提交 · Esc 取消</Text>
- </Box>

+ <Box borderStyle="round" borderColor="warning">
+   <Text color="warning" bold>▸ ask_user · 等待输入</Text>
+ </Box>
```

### 11.2 ToolCard（TUI）改造

```diff
- CATEGORY_COLOR = {
-   thinking: 'magenta', file: 'blue', command: 'yellow',
-   code: 'yellow', graph: 'green', subagent: 'cyan',
-   plan: 'yellow', workflow: 'magenta', other: 'gray',
- } // 6 色

+ const ROLE_COLOR = {
+   interaction: 'warning',
+   execution: 'accent',
+   thinking: 'mauve',
+   done: 'textMuted',
+   error: 'error',
+ } // 3 色

- <Text color="gray" dimColor>── Input ──</Text>
+ <Text color="textMuted" bold>Input</Text>

- <Text color="gray" dimColor italic>running...</Text>
+ <ShimmerText>{title}</ShimmerText>  // 标题本身就是流光
```

### 11.3 PermissionCard（React 共享）改造

```diff
- <span>🔒 权限审批</span>
- <span style={{ backgroundColor: '#dc2626' }}>YOLO</span>
- <span>等待你的确认</span>

+ <span>▸ permission</span>
+ <span className="permission-level-badge">STD</span>
+ <span className="permission-status">等待确认</span>
```

---

## 12. 违规检查清单

PR review 时按此清单逐条勾选：

### 颜色
- [ ] 没有硬编码 hex（必须用 token）
- [ ] 没有 inline style 写颜色
- [ ] 卡片边框色符合 §3 角色色分类
- [ ] 没有 6+ 种颜色滥用

### 排版
- [ ] 标题格式符合 §6（前缀符号 + 类别 · 状态 · badge）
- [ ] spacing 在 8 倍数节奏上（4/8/12/16/24）
- [ ] 没有 emoji 装饰符（🔒 ✓ ⚠ 💡 🚀 ⠋ 等）
- [ ] 没有 ASCII 分隔符（`---` `===` `───`）
- [ ] 没有 `italic` 修饰
- [ ] 没有 `dimColor` 滥用
- [ ] 没有 `press ESC to close` 这类提示

### Loading
- [ ] 写类工具（write/edit/ast_edit）有流光
- [ ] 执行类工具（bash/launch）有流光
- [ ] 读类工具（read/grep/glob）有流光
- [ ] LSP / code_graph 工具**用户主动调用时**有流光；后台调用不强求
- [ ] thinking 卡片有流光
- [ ] 流光在完成后立即停止

### 一致性
- [ ] 三端使用相同的 token 名
- [ ] TUI 用 `TUI_COLORS` 导出色名，不直接用 `color="cyan"` 字符串
- [ ] Desktop/Mobile 用 `var(--*)` 变量，不写 hex
- [ ] 按钮尺寸符合 §9 对齐表
- [ ] 圆角符合 §9 对齐表

### 文案
- [ ] 没有"AI 味"装饰文案（§10.1 表）
- [ ] 状态描述在 `·` 之后
- [ ] 不解释用户已知的行为

---

## 附录 A：TUI 颜色对照速查

| Token | TUI 色名 | CSS hex |
|-------|----------|---------|
| accent | `cyan` | `#89b4fa` |
| success | `green` | `#a6e3a1` |
| warning | `yellow` | `#f9e2af` |
| error | `red` | `#f38ba8` |
| mauve | `magenta` | `#cba6f7` |
| text-primary | `white` | `#cdd6f4` |
| text-secondary | `gray` | `#a6adc8` |
| text-muted | `gray` | `#6c7086` |

## 附录 B：状态前缀字符

```
▸  待操作
▶  执行中
✓  已完成
✗  失败
?  待审批
```

## 附录 C：变更历史

| 日期 | 改动 | 作者 |
|------|------|------|
| 2026-07-31 | 初版 | — |