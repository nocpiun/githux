# 2026-11-09 NSH Accounts / Settings 原位替换

本地任务 `nsh-accounts`，基线 `01decd2c6aa1331d0172d6281875a2bd7649828f`，分支 `migration/nsh-accounts`。仅本地实现/提交供集成作者摘取，不 push、建 PR、改 PR #75 或合入 main。执行依据为 [原位契约](../native-shell-inplace.md)；共同 API 与当前行为归 [Settings](../settings.md) 维护。

## 实现

- 原位替换 `src/ui/accounts/**`、`src/ui/settings/**`，删除旧 Ely form/binding/layout 辅助文件并用实际 NSH 控件接线，保留旧实现于 Git 基线。
- `AccountsView` / `SettingsView` 消费同一个 shell-owned `app::runtime::RuntimeHandle`，真实事件只写本地 effect 队列；UI poll 提交 Command、取消、接收权威 snapshot，正文渲染无 I/O。没有新建生产账户服务、数据库、HTTP 或业务缓存。
- 保存账户、当前标记/切换、Classic PAT 添加/同身份替换/重新验证、确认移除和显式恢复清理已接 Runtime。零账户、已显示仓库撤权及 retire 都清空旧展示；重复命令在排队即被门禁拦截；取消不宣称回滚，保持 settling，完成后对账。
- 输入保持 masked 并支持 Show/Hide；提交/取消/表单切换清空临时秘密并换新 keyed native input，避免保留旧编辑历史。Enter 用原生 SubmitEditing，组合态受保护；Esc 在非组合态取消，完成/取消通过 ElementRef 返回发起点。PAT 不进入 presentation/snapshot/日志。
- Settings 仍只有 Accounts；宽态紧凑分类行与左侧 indicator、窄态默认 NSH Tabs/underline，正文纵向滚动。保留字体与布局结构，使用 NSH 默认语义色/控件。
- 真实头像 provider 由集成作者注入；`AccountAvatar` 只允许准备好的本地图片字节变为 data source，不回退为远程 URL。每次 poll 取当前已授权图片，None 删除旧图；未接 provider 保持圆形 initials，不能据此宣称真实头像完成。
- Accounts example 使用生产 Runtime；Settings example 在窗口前以临时目录/内存凭据/真实服务/重复可请求传输建立两个合成账户，正常窗口不构造第二套账户组装。两者使用最终 library 路径并等 ExitGate 后退出。

## 验证边界与证据

本地基线尚未包含 integrator 的最终 app/runtime、foundation、workspace 与 library 注册。忽略目录 `.agents/task/probe` 仅提供最终路径 alias 和文档导航类型，编译实际拥有的生产文件与真实 services/storage/cache，examples 使用同一 library 类型。探针通过不等于默认产品通过；根 Cargo/lock/main/lib、旧共享 app/accounts 及其测试均未修改。

当前独立探针已验证的主要场景：空输入不提交；排队重复提交只一个 Command；PAT 掩码与提交清空；非当前账户替换、错身份拒绝；真实添加/切换/重验证；失败后无关重验证不会关闭后来修正的表单；自动仓库读取不重播旧完成结果/不重复请求；取消后的 busy/settling 与权威对账；撤权单次 poll 清空已显示列表；唯一账户移除后的零账户清屏；宽窄导航与实际选中状态。

## 最终验证

- `cargo fmt --manifest-path .agents/task/probe/Cargo.toml --all -- --check`：通过（run `47605509-55b6-4514-bd08-0928c20c275d`，0 exit，618ms）。
- `cargo clippy --offline --manifest-path .agents/task/probe/Cargo.toml --all-targets -- -D warnings`：通过（run `92ab11fc-efff-47bc-92b5-8f75b8fde181` / `b3d38b23-83f3-49aa-a500-b7ad29581d3b`，0 exit，0 警告）。
- `cargo build --offline --manifest-path .agents/task/probe/Cargo.toml --examples`：通过（run `4d242b52-556a-481c-9689-af1b1f22a318`，0 exit，构建 account 与 settings 两示例）。
- `cargo test --offline --manifest-path .agents/task/probe/Cargo.toml --lib -- accounts_impl settings_impl runtime_impl::tests`：通过（run `ba03f149-623a-4306-bd1b-7d0b13183b73`，32 passed / 0 failed，耗时 37.45s）。
- `cargo test --offline --manifest-path .agents/task/probe/Cargo.toml --lib network::transport::bounded_tests::chunked_and_no_length_stop_before_eof_on_success_and_errors`：通过（run `26170ae5-0039-483c-9445-55e05b4eefc5`，1 passed / 0 failed，核对此前偶发 socket 失败在纯离线环境恢复，证实非数据层代码回归）。
- `git diff --check`：通过，无空白异常。


未执行真实 PAT/带凭据 GitHub、Computer Use、截图、真实 Windows IME/焦点/滚动/DPI 操作或其他平台构建。最终完整默认入口、shell 组合、真实头像缓存与负责人视觉验收由集成作者/负责人完成。

## 旧测试映射

旧 `src/ui/accounts/form.rs` / orphan `view.rs` 无内联行为测试。旧 `src/app/accounts/tests.rs` 全部留在 Git、未删除/禁用，但其 GPUI controller 与正式 Shell 装配待 integrator 替换；本分支不为旧 UI API 提供虚假兼容层。其服务/存储回归仍保留；workspace、真实头像/缓存、退出与滚动的正式 Shell 联合测试归集成作者，不以局部探针替代。

| 被替换的 Settings 测试 | NSH 对应证据 / 边界 |
| --- | --- |
| `authorization_distinguishes_missing_expired_and_connectivity` | `authorization_distinguishes_saved_missing_rejected_and_connectivity` 验证 pending/missing/expired/connectivity/verified 的区分 |
| `contextual_form_masks_clears_and_returns_focus` | headless 掩码登录、组合态与旧原生输入节点销毁；取消/成功收起通过 ElementRef 返回；真实 Windows 焦点仍需验收 |
| `replacing_inactive_account_uses_authoritative_target_and_switch_omits_current` | controller chosen-target + `inactive_replacement_rejects_another_identity_then_commits_only_the_target`，Current 行不生成 Switch |
| `add_switch_and_revalidate_publish_authoritative_service_state` | 真实服务登录 A/B、切回 A、重验 B 的 epoch/active 断言 |
| `a_startup_repository_failure_is_visible_before_any_user_action` | 直接消费 Runtime RepositoryList::Failed 的 compact warning/retry；startup error 与恢复状态分别呈现，初始化恢复状态依赖 integrator Runtime 填充 |
| `committed_form_closes_even_when_the_follow_up_repository_read_fails` | inactive replacement 真实服务测试：替换提交后 repository 403，表单关闭且不会报 PAT 未保存 |
| `a_failed_submission_is_not_completed_by_a_later_unrelated_revalidation` | mismatch 后输入修正，再重验另一操作，表单/文字仍保留 |
| `double_submit_and_cancellation_use_existing_controller` | 排队重复提交只一个 Command；gated 真实 Runtime 取消重验证仍 busy/settling 并最终对账 |
| `ime_confirmation_does_not_submit_or_close_form` | headless 注入真实 CompositionChange/SubmitEditing 验证组合态不提交；Windows 原生 IME 不由注入证明 |
| `narrow_tabs_scroll_without_content_overflow_and_keyboard_reveals_last` | 原生 NSH horizontal Scroll + Tabs、窄态不存在宽指示线、真实 Tab 选中；滚动/键盘 reveal 的原生体验仍需验收 |
| `wide_categories_are_compact_with_one_indicator_and_follow_selection` | headless 测量实际 Tab 高度 32px，单 indicator，点击后选中态改变 |

没有新增可复用规则：以上已按 L002/L012/L016/L017/L022/L023 的现有边界处理，故不修改 LESSONS。
