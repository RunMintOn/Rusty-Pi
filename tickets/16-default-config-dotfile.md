# Ticket 16 — 默认配置 dotfile

## 现状

- 每次运行必须传 `-p deepseek -m deepseek-v4-pro`（或全部用默认值 mock）
- 没有配置文件保存用户偏好
- 原版有庞大的 config 系统（`config.ts` 566 行），管理安装方式检测、包管理、路径解析等下，与我们需求无关

## 目标

`~/.rusty-pi.toml`（或 `~/.pi/config.toml`）提供简洁的默认配置：

```toml
default_provider = "deepseek"
default_model = "deepseek-v4-flash"
# 可选
prompt_paths = ["~/my-templates"]
skill_paths = ["~/my-skills"]
```

优先级：CLI 参数 > 环境变量 > 配置文件 > 内置默认值。

## Blocked by

None（完全独立，~100 行代码）

## 设计要点

### 1. 配置文件路径

```rust
fn config_paths() -> Vec<PathBuf> {
    vec![
        get_agent_dir().join("config.toml"),
        dirs::home_dir().unwrap().join(".rusty-pi.toml"),
        PathBuf::from(".rusty-pi.toml"),
    ]
}
```

优先级顺序：列出的顺序，后面的覆盖前面的。

### 2. 配置结构

```rust
#[derive(Debug, Default, Deserialize)]
struct RustyPiConfig {
    default_provider: Option<String>,
    default_model: Option<String>,
    prompt_paths: Option<Vec<String>>,
    skill_paths: Option<Vec<String>>,
}
```

使用 `toml` crate（轻量，纯 Rust，无需 `serde_yaml`）。

### 3. CLI 集成

在 `main.rs` 中，解析 CLI 参数前先加载配置文件。CLI 参数（`-p`、`--model` 等）优先覆盖配置值。

### 4. 什么不做

- 不做配置写入（用户手动编辑文件）
- 不做配置校验之外的复杂逻辑
- 不做 profile/多环境支持
- 不做 provider-specific 配置

## 测试策略

- 解析有效 TOML 配置
- 空配置/不存在文件回退默认值
- CLI 参数优先于配置文件
- 多级配置合并

## 文件改动清单

| 文件 | 改动 |
|---|---|
| `src/main.rs` | 添加配置加载逻辑；`-p` 默认值从 `"mock"` 改为读取配置 |
| `Cargo.toml` | 添加 `toml` 依赖 |
