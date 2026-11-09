# Native Shell 使用默认样式

- 日期：2026-11-09；执行者：主 Agent；本地 migration/native-shell，不 push/PR、不合 main。
- 负责人明确迁移无需复刻 Ely，使用 NSH 默认样式。此决定覆盖初始迁移的外观基线，功能、信息结构、字体与已确认交互仍保留。

## 改动

- Root 从 nsh_base::tokens_of(None) 获取自带默认主题，仅追加 Githux UI（Noto Sans/SC）和 Google Sans Code 字体族；删除 Ely 调色板与颜色覆盖。
- 基础表面、文字、tooltip、状态背景、边框使用 NSH 语义 token，圆角使用默认 radius；移除旧 island 固定圆角常量。
- 普通/图标按钮采用 NSH Button 的原生按钮行为，保留最小布局；NSH v0.4.0 Button 本身无预制外观，不把 gallery 的自定义主题当成默认。黄色 warning 的状态语义按既有负责人约定继续保留。
- 移除 Windows 入口强制 Dark，使用 NSH 默认 System 外观。
- 更新迁移分支 AGENTS、迁移计划、README 与 Reader 文档，后续迁移以该决定为准。

## 验证

所有 Cargo 经 scripts/verify.mjs，dev/test debug=0、incremental=0。

- fmt：bc63062f，通过。
- cargo clippy -p githux-nsh --all-targets --locked --offline -- -D warnings：aa66abeb，通过。
- cargo test -p githux-nsh --lib tests::foundation --locked --offline：172c31e1，3/3 通过（控件、字体族请求、联合 Reader/Source 切换）。未改业务逻辑，不重复整套业务回归或新增样式镜像测试。
- cargo build -p githux-nsh --bin githux-nsh --locked --offline：3b95be8c，通过，可执行文件已更新。
- 未使用 Computer Use、未启动或截图验收。最终效果由负责人验收，原启动命令不变。