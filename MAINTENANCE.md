# Maintenance

## Build & Test

```bash
cd rusty-pi && cargo test && cargo clippy
```

71 tests，全部本地 mock，不碰任何在线 API。

## Available Tools

当前注册的 4 个工具实现在 `src/coding_agent/tools/`，每个实现 `Tool` + `AgentTool` trait 后在 `main.rs` 注册。详细行为见各模块源码。

| Tool | 文件 |
|---|---|
| `bash` | `tools/bash.rs` |
| `read` | `tools/read.rs` |
| `write` | `tools/write.rs` |
| `edit` | `tools/edit.rs` |

### 添加新工具

每个工具只需：

1. 在 `tools/` 下新建 `.rs` 文件
2. 实现 `Tool` trait（`name`/`description`/`parameters`）
3. 实现 `AgentTool` trait（`label`/`execute`，可选 `prepare_arguments`/`execution_mode`）
4. 在 `tools/mod.rs` 声明 `pub mod your_tool;`
5. 在 `main.rs` 注册：`let tool = YourTool::new(cwd);` → 加入 `tools: vec![...]`

## 测试陷阱

### `Content` 没有 `Display`

`ai::types::Content` 枚举没有实现 `Display`/`ToString`。测试中不能直接 `assert!(result.content[0].to_string()...)`，必须用模式匹配：

```rust
let text = match &result.content[0] {
    Content::Text { text } => text.as_str(),
    _ => panic!("Expected text content"),
};
```

## 更新 Reference

```bash
cd reference/earendil-works-pi
git fetch --depth 1 && git reset --hard origin/main && rm -rf .git
```

## Git 陷阱

### filter-branch 物理删除文件

`git filter-branch --index-filter` 结束后会重置工作树。被 `--index-filter` 从所有 commit 中移除的文件会从磁盘消失。安全做法：

- 备份后再操作，或
- 使用 `git filter-repo`（无此副作用），或
- 不重写历史，直接 `git rm --cached` + 新 commit。
