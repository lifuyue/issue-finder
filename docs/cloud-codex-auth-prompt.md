# Cloud Codex 显式回退认证配置交接

本交接仅适用于显式选择的 Codex 回退。默认主 provider 是阿里云
`aliyun_decision`，另可选 `cloudflare_clef_flash`；其凭据与 endpoint/account
配置见 [原生 decision provider 指南](decision-providers.md)。原生 provider
不使用 `decision-auth-init`，失败也不会自动转用这里的 Codex 登录。

要让 scout 使用 Codex，应在 Issue Finder 的配置中明确设置：

```toml
[decision]
provider = "codex"
```

单独运行 `decision-check --provider codex` 只验收 Codex，不改变 scout 的配置。

使用两个相互独立的配置项：

| 配置项 | 类型 | 内容 |
| --- | --- | --- |
| `ISSUE_FINDER_CODEX_AUTH_JSON` | 受保护的运行时环境变量 | 完整、原始的 Codex `auth.json` JSON 文本；不做 base64 编码 |
| `ISSUE_FINDER_CODEX_HOME` | 可选的普通环境变量 | 本 task 独享的绝对路径，例如 `/tmp/issue-finder-codex-home` |

用户在 Cloud 的安全配置界面填写真实 JSON，不把内容发给配置 task。
来源应是自己完成正常 CLI 文件登录后产生的有效 `auth.json`，不能是平台主
Agent 的内部认证，也不能是依赖外部续期的 `chatgptAuthTokens` 缓存。
默认登录目录通常为 `~/.codex`；显式使用 `CODEX_HOME` 或系统钥匙串时位置和
存储方式不同。需要另行生成文件登录时，可在自己信任的机器上为独立的 Codex
home 使用 `cli_auth_credentials_store="file"` 完成登录，不覆盖主 Agent 的目录。

这个值必须通过能交付真实内容的受保护 **Environment variables** 机制提供。
Network secrets 的代理占位符不能写成可用的 `auth.json`。如果当前配置界面只能
提供占位符，报告不支持，不将凭据改存到明文说明、配置草稿或仓库。

以下内容可直接交给环境配置 task；不含任何真实凭据。当前只完成代码和离线
协议校验不能视为真实服务验收；不能因失败自行启用付费计划或改选 provider。

---

请为当前 Issue Finder Cloud 环境配置显式 Codex 回退及其 CLI 文件认证注入，沿用已有环境，
保留其网络、代理、CA、GitHub 凭据、安装路径和 session v2 启动契约。

1. 先读取当前环境及已发布的配置版本，区分已生效版本和编辑中的草稿。新增受保护
   的运行时环境变量 `ISSUE_FINDER_CODEX_AUTH_JSON`，由我在安全界面填入完整原始
   `auth.json`。不要在聊天、日志、命令参数、配置说明或仓库里写入其值，也不要读取
   平台主 Agent 的认证文件。不要用 Network secret 占位符代替 JSON，不新增
   `GITHUB_TOKEN` 别名。若尚未填值，明确报告待注入，不能伪报验收成功。
2. 增加普通变量 `ISSUE_FINDER_CODEX_HOME=/tmp/issue-finder-codex-home`，要求该目录
   在不同 task 间隔离、在同一 task 内可写并保留刷新结果。不全局修改 `CODEX_HOME`；
   Issue Finder 会只为自己的 Codex 子进程指定这个目录和 file credential store。
   不把认证文件、运行期状态或真实 secret 烘焙到 Cloud 快照。
3. 在启动指引中保留初始化前记录仓库 HEAD、分支和工作区状态、fetch main、保留已有
   改动的同步规则。失败即停止，不运行遗留的 `issue-finder-setup-command.sh`。
   明确配置 `[decision].provider = "codex"`，不因原生服务失败自动切换。
   从同步源码安装 Issue Finder 后，先检查 `issue-finder decision-auth-init --help`。
   **仅同步 main 不代表包含新功能**：如该命令不存在，应停止并报告所需代码尚未进入
   安装版本，不改用旧发布包冒充支持。Codex CLI 使用现有兼容安装；需安装时按仓库脚本
   显式选用已验证的 `0.159.3`，不得覆盖主 Agent 的 Codex 安装。
4. 将以下两步放在每个新 task 的运行期初始化中，不能只放在镜像构建阶段：

   ```bash
   issue-finder decision-auth-init
   issue-finder decision-check --provider codex
   ```

   脚本应失败即停止。需要指定 Codex 路径时用 `ISSUE_FINDER_CODEX_BIN` 或
   `decision-check --codex-binary /absolute/path/to/codex`。也可用仓库脚本
   `scripts/decision-codex.sh --check-only --auth-from-env` 完成两步。
   `decision-auth-init` 创建权限 `0700` 的目录和 `0600` 的文件，默认保留已有刷新后
   文件；不要在常规启动中加入 `--replace`。只有明确轮换种子且没有 决策模型 调用
   运行时，才执行 `issue-finder decision-auth-init --replace`。
5. 保持 `sessionContractVersion: 2`，恰好只有 `scout`、`assess` 两个业务工具。
   `decision-auth-init` 和 `decision-check` 是环境初始化命令，不加入业务工具清单。
   新 shell 应能直接找到 Issue Finder，并通过 `issue-finder tools list` 核验契约。
6. 用真实 `decision-check --provider codex` 请求验收：模型 `gpt-6-luna`，推理 `none`，独立上下文、
   禁用工具、JSON Schema 与应用层校验。初始化成功或 `codex login status` 不能证明
   认证和模型访问成功；认证失败、额度或模型错误分别报告，不覆盖后继续声称成功。
   只报告脱敏状态，绝不输出认证 JSON、token、Authorization header 或全部环境变量。
7. 完成配置修改，并按环境配置工作流保存、发布。报告最终发布版本及新 task 的实际
   绑定版本、源码提交、CLI 路径、初始化结果、真实请求结果和两工具契约；无法验证的
   项目明确标为无法验证，不把配置草稿当成已生效状态。

请同时在环境说明中记录此方案的限制：Codex 自动续期只更新运行期文件，不会反写
Cloud 保存的静态 JSON。新建环境必须使用当前有效种子；同一刷新凭据不能分发给多个
并行 task/runner，登录源机器也不应继续消费同一刷新令牌。每个独立 runner 应使用
独立凭据或受支持的串行凭据持久化机制。即使任务串行，环境销毁后若没有安全保存最新
刷新结果，旧种子也可能失效。此版本不提供跨 task 凭据回写；需要重新安全注入最新值，
不能承诺一次配置永久登录。同一 task 内单个 app-server 的四路 决策模型 请求保持现状。

---

实现与边界见 [决策模型 认证说明](decision.md#injected-codex-authentication)、
[安装说明](../skills/issue-finder-cli/references/install.md#codex-authentication) 和
[Codex 官方 CI/CD 认证说明](https://developers.openai.com/codex/auth/ci-cd-auth)。
