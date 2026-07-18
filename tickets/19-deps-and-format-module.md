# Ticket 19 — 添加 sparcli + inquire 依赖 + format/ 模块骨架

## 现状

- 当前依赖（Cargo.toml）没有 sparcli 和 inquire
- 输出格式靠手写 ANSI 字符串（如 diff 渲染、错误显示），散落在各处
- 没有统一的格式化层

## 目标

1. 在 Cargo.toml 添加 sparcli 和 inquire
2. 创建 `src/format/` 目录，导出模块
3. 实现 `OutputFormatter` 骨架：至少包含 `session_info()` 和 `error()` 两个函数
4. 每个函数附带测试（用 `print_to` 捕获输出断言）

## 设计

参考 `spec-bare-terminal-architecture.md` 和 `crate-reference-bare-terminal.md`。

### 依赖

```toml
[dependencies]
sparcli = "0.3"
inquire = "0.9"
```

### 模块结构

```
src/format/
├── mod.rs      ← pub mod 声明 + pub use OutputFormatter
└── out.rs      ← OutputFormatter 结构体 + 方法
```

### OutputFormatter（初始版）

```rust
pub struct OutputFormatter {
    theme: sparcli::Theme,
}

impl OutputFormatter {
    pub fn new() -> Self { Self { theme: Theme::default() } }

    /// 显示当前 session 信息（KV 列表）
    pub fn session_info(&self, info: &SessionInfo) -> String {
        let mut buf = Vec::new();
        KeyValueList::new()
            .entry("Session", &info.id)
            .entry("Model", &info.model)
            .entry("Messages", &info.msg_count.to_string())
            .entry("CWD", &info.cwd)
            .print_to(&mut buf)
            .unwrap();
        String::from_utf8_lossy(&buf).to_string()
    }

    /// 错误信息（红色 Alert）
    pub fn error(&self, msg: &str) -> String {
        let mut buf = Vec::new();
        Alert::error(msg).print_to(&mut buf).unwrap();
        String::from_utf8_lossy(&buf).to_string()
    }
}
```

## 测试

```rust
#[test]
fn test_session_info_contains_id() {
    let fmt = OutputFormatter::new();
    let out = fmt.session_info(&SessionInfo {
        id: "test-123".into(), model: "m".into(), msg_count: 0, cwd: "/".into()
    });
    assert!(out.contains("test-123"));
}

#[test]
fn test_error_output() {
    let fmt = OutputFormatter::new();
    let out = fmt.error("something broke");
    assert!(out.contains("something broke"));
}
```

## 文件改动

| 文件 | 改动 |
|---|---|
| `Cargo.toml` | 添加 sparcli, inquire |
| `src/format/mod.rs` | 新建，导出 OutputFormatter |
| `src/format/out.rs` | 新建，OutputFormatter 实现 |
| `src/lib.rs` | 添加 `pub mod format;` |
