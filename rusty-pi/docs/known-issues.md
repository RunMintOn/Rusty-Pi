# Known Issues & Solutions

## 测试并行运行时挂起 (已修复)

**症状**: `cargo test` 并行运行时 50%+ 概率挂死，单线程正常。

**根因**: `tokio::process::Child::drop` 不调用 `waitpid`。Runtime 被 drop 时子进程变 zombie，阻塞 `do_wait()`。

**修复**: bash 工具用 `std::process::Command` + OS 线程 blocking I/O 和 `waitpid`，绕过 tokio 进程管理。

**防范规则**:
- 任何需要 spawn 子进程并等待的代码，**不要用 `tokio::process::Command`**，用 `std::process::Command` + OS 线程。
- 如果必须用 tokio process，确保在所有代码路径上都调用 `child.wait().await`，且不能被 runtime drop 取消。
- 并行测试挂起时，先 `ps -eo pid,ppid,stat,comm | grep " Z "` 检查 zombie，再查 `pstree -p` 找残留子进程。
