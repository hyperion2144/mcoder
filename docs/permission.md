# 权限系统（Permission System）

设计文档 §8.8：三级权限控制，agent 执行敏感工具前必须经用户审批。

## 三个级别

| Level | 行为 | UI 标识 |
|-------|------|---------|
| **Yolo**（最高权限） | 全部自动执行，无需审批 | 🔴 YOLO 徽章 |
| **Standard**（默认） | 只读工具自动；写/执行类需审批 | 🟡 STD 徽章 |
| **Strict**（最保守） | 所有非只读工具都需审批 | 🔵 STRICT 徽章 |

## 工具分类

**自动通过（只读）：**
`read`, `grep`, `glob`, `lsp_*`, `todo_read`, `memory_*`, `code_graph_*`,
`workflow_read`, `workflow_state`, `view_image`, `session_list`,
`session_snapshot`, `model_list`, `role_list`, `ask_user`, `plan_read`, `ast_query`

**需要审批（写/执行）：**
`write`, `edit`, `ast_edit`, `bash`, `launch`, `mcp_*`,
`browser_*`, `screen_*`, `app_*` 等

## 配置示例

### 全局配置（`~/.mcoder/config.toml`）

```toml
[permission]
# 权限级别：yolo / standard / strict
level = "standard"

# yolo mode 仍拒绝的工具（兜底白名单；如未审计的 mcp_* 工具）
yolo_deny = []

# strict mode 时额外审批的工具列表
# 留空时审批所有非只读工具；填写后只审批列表内的工具
strict_require_approval = []

# strict mode 时额外自动通过的工具
strict_auto = []
```

### 项目级覆盖（`.mcoder/permission.toml`）

```toml
# 覆盖全局配置
level = "yolo"

# 这个项目测试需要 bash 写入，本项目额外 yolo 放行
yolo_deny = ["rm -rf", "sudo"]
```

**优先级**：项目级 > 全局（项目级存在时覆盖全局）。

## RPC API

### 服务端 → 客户端（notification）

```json
{
  "jsonrpc": "2.0",
  "method": "permission.pending",
  "params": {
    "session_id": "...",
    "request": {
      "request_id": "uuid",
      "session_id": "...",
      "tool_call_id": "toolu_xxx",
      "tool_name": "bash",
      "tool_args": {"command": "rm -rf build/"},
      "reason": "tool 'bash' modifies state; confirm to execute",
      "level": "standard"
    }
  }
}

{
  "jsonrpc": "2.0",
  "method": "permission.resolved",
  "params": {
    "session_id": "...",
    "request_id": "uuid",
    "decision": {"type": "Allow"}
  }
}
```

### 客户端 → 服务端（request）

```json
{
  "jsonrpc": "2.0",
  "id": 123,
  "method": "permission.submit",
  "params": {
    "session_id": "...",
    "response": {
      "request_id": "uuid",
      "session_id": "...",
      "decision": {"type": "Allow"}
      // 或 {"type": "Deny", "reason": "denied by user"}
      // 或 {"type": "AlwaysAllow"}
    }
  }
}
```

## 三端 UI

### TUI
- 消息流中内联 `PermissionCard`（紫边框）
- 按键：`A`=Allow · `D`=Deny · `Y`=Always Allow · `Esc`=Deny

### Desktop
- 消息流中内联卡片（紫色 header + 三个按钮）
- 鼠标点击 `Allow` / `Deny` / `Always Allow`

### Mobile
- 同 Desktop，但按钮更大便于触摸
- 滑动卡片可隐藏历史决议

## 设计要点

1. **服务端为 source of truth**：所有权限决策在服务端执行，客户端只负责展示 + 收集决策
2. **超时机制**：60s 未决议自动 deny
3. **多客户端同步**：决策广播到所有连接的 client
4. **取消传播**：session cancel 时所有 pending permission 自动取消
5. **AlwaysAllow 语义**：仅 standard/strict 模式生效；yolo 模式无效（已经是 always）