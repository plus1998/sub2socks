# sub2socks

一个轻量级、自托管的代理订阅与 SOCKS5 账号管理面板。它从订阅中同步代理节点，通过 [Mihomo](https://github.com/MetaCubeX/mihomo) 为每个账号绑定指定节点，并在一个统一的 SOCKS5 端口上按用户名分流。

> 项目当前处于早期开发阶段，适合个人、本机或受信任内网使用。管理后台尚未实现完整的登录鉴权，请先阅读[安全说明](#安全说明)，不要将管理端口直接暴露到公网。

## 功能特性

- Web 管理界面，支持中英文切换
- 首次启动初始化引导
- 远程订阅添加、同步和删除
- 节点列表、启用/停用和删除
- SOCKS5 账号创建、编辑、启用/停用和删除
- 单一公网 SOCKS5 端口，多账号共用
- 根据 SOCKS5 用户名将流量转发到账号绑定的代理节点
- 自动生成 Mihomo 配置并管理 Mihomo 进程
- SQLite 持久化，无需额外数据库
- 支持 Docker Compose 部署

## 工作原理

```text
客户端
  │
  │ SOCKS5 用户名/密码
  ▼
统一入口 :9999
  │
  ├─ 用户 alice ──► Mihomo 内部监听端口 ──► 节点 A
  ├─ 用户 bob   ──► Mihomo 内部监听端口 ──► 节点 B
  └─ 用户 carol ─► Mihomo 内部监听端口 ──► 节点 C
```

应用包含两个主要入口：

| 入口 | 默认端口 | 用途 |
| --- | ---: | --- |
| Web 管理界面/API | `3000` | 管理订阅、节点、SOCKS 账号和 Mihomo |
| SOCKS5 服务 | `9999` | 客户端统一连接入口，必须使用用户名/密码认证 |

每个启用的 SOCKS 账号会获得一个从 `50001` 开始分配的内部端口。内部监听器仅绑定 `127.0.0.1`，外部客户端只需要连接统一的 `9999` 端口。

## 支持的订阅格式

项目会自动尝试解析：

- Clash/Mihomo YAML（包含 `proxies` 列表）
- Base64 编码的节点 URI 列表
- 纯文本节点 URI 列表

当前 URI 解析支持：

- `http://`
- `socks5://`
- `trojan://`
- `vless://`（包含常用 TLS、Reality、WebSocket 和 gRPC 参数）

Clash/Mihomo YAML 中的节点会尽量保留原始节点字段，因此通常比单条 URI 能保留更完整的高级配置。不同订阅提供方的格式存在差异，部署后请使用实际订阅验证节点连通性。

## 快速开始

### 环境要求

推荐使用 Docker 部署：

- Docker Engine 20.10+
- Docker Compose v2
- Linux `x86_64/amd64`
- 至少约 1 GB 可用内存用于首次镜像构建

仓库当前附带的是 Linux amd64 Mihomo 二进制，`docker-compose.yml` 也固定使用 `linux/amd64`。ARM64 服务器需要自行替换对应架构的 Mihomo，并移除或修改 `platform` 配置。

### 使用 Docker Compose

```bash
git clone <repository-url> sub2socks
cd sub2socks
docker compose up -d --build
```

查看容器状态：

```bash
docker compose ps
```

查看日志：

```bash
docker compose logs -f --tail=100
```

正常启动时可看到类似输出：

```text
SOCKS5 multiplexer listening on 0.0.0.0:9999
Rust Proxy Manager listening on http://0.0.0.0:3000
```

打开：

```text
http://服务器地址:3000
```

首次访问时完成管理员信息初始化，然后按照[使用教程](#使用教程)添加订阅和 SOCKS5 账号。

> 初始化信息目前只用于记录初始化状态，尚未对管理 API 实施访问控制。公网部署必须在反向代理层添加认证或 IP 白名单。

### 从源码运行

本地开发需要：

- Rust 1.75 或更高版本
- 可执行的 Mihomo 二进制

```bash
export PORT=3000
export SOCKS_PORT=9999
export RUST_PROXY_MANAGER_DB=./proxy_manager.db
export RUST_PROXY_MANAGER_DATA_DIR=./data
export MIHOMO_BINARY=/path/to/mihomo

cargo run
```

如果没有设置 `MIHOMO_BINARY`，应用还会依次尝试：

1. 构建时通过 `MIHOMO_EMBED_PATH` 嵌入的二进制
2. `PATH` 中的 `mihomo`
3. 与应用可执行文件位于同一目录的 `mihomo`

仅启动 Web 服务不要求 Mihomo 立即运行，但在点击“启动 Mihomo”前必须提供可执行的 Mihomo 二进制。

## 使用教程

### 1. 初始化

首次打开管理界面后，填写管理员用户名和密码并提交。初始化完成后会进入管理面板。

请注意：当前版本不会创建登录会话，也不会拦截未认证的 API 请求。这里设置的密码不是管理后台的有效安全边界。

### 2. 添加订阅

在“订阅”区域填写：

- 名称：便于识别的订阅名称，可选
- URL：订阅提供方给出的完整地址

提交后应用会立即下载并解析订阅。同步成功后，节点会显示在节点列表中。

如果同步失败：

- 检查服务器能否访问订阅地址
- 检查 URL 是否完整、是否已过期
- 检查订阅格式是否受支持
- 查看容器日志获取详细错误

当前行为是先保存订阅记录再同步，因此首次同步失败的记录仍可能保留在列表中，可以稍后重试或手动删除。

### 3. 管理节点

节点同步后，可以：

- 启用或停用节点
- 删除单个节点
- 重新同步订阅以更新节点

只有启用的节点会写入 Mihomo 配置，也只有启用的节点可以在创建 SOCKS 账号时选择。

### 4. 创建 SOCKS5 账号

在“SOCKS 账号”区域填写：

- 名称：账号备注
- 用户名：客户端连接 SOCKS5 时使用，必须唯一
- 密码：客户端连接 SOCKS5 时使用
- 节点：该账号固定使用的上游代理节点

所有账号共享同一个外部 SOCKS5 端口。应用根据用户名找到对应账号，再将连接交给该账号绑定的节点。

添加或修改账号后，需要重新启动 Mihomo 才能让生成的配置生效。

### 5. 启动 Mihomo

在管理界面点击“启动 Mihomo”。状态显示为运行中后，即可使用 SOCKS5 服务。

也可以通过本机 API 检查状态：

```bash
curl http://127.0.0.1:3000/api/mihomo/status
```

### 6. 测试 SOCKS5

使用 `curl` 验证出口 IP：

```bash
curl \
  --proxy 'socks5h://用户名:密码@服务器IP:9999' \
  https://api.ipify.org
```

查看更完整的出口信息：

```bash
curl \
  --proxy 'socks5h://用户名:密码@服务器IP:9999' \
  https://ipinfo.io/json
```

推荐使用 `socks5h://`，以便域名解析也通过代理完成。

如果用户名或密码包含 `@`、`:`、`/` 等特殊字符，需要先进行 URL 编码，或者在支持独立填写代理认证信息的客户端中配置。

## 在 1Panel 上部署

### 1. 检查服务器架构

在 1Panel 终端执行：

```bash
uname -m
```

当前项目可以直接部署在输出为 `x86_64` 的服务器上。

### 2. 上传项目

将项目上传或克隆到服务器，例如：

```text
/opt/1panel/apps/local/sub2socks
```

项目目录至少应包含：

```text
Cargo.toml
Cargo.lock
build.rs
Dockerfile
docker-compose.yml
mihomo
src/
```

### 3. 使用安全的端口映射

项目自带的 Compose 会将管理端口映射到所有网卡。公网部署时，建议在 1Panel 编排中改用以下配置：

```yaml
services:
  rust-proxy-manager:
    platform: linux/amd64
    build:
      context: .
      dockerfile: Dockerfile
    image: rust-proxy-manager:latest
    container_name: rust-proxy-manager
    restart: unless-stopped
    ports:
      # 管理端仅监听宿主机回环地址，由 1Panel 反向代理访问
      - "127.0.0.1:3000:3000"
      # SOCKS5 对外服务端口
      - "9999:9999"
    environment:
      PORT: "3000"
      SOCKS_PORT: "9999"
      RUST_PROXY_MANAGER_DB: /data/proxy_manager.db
      RUST_PROXY_MANAGER_DATA_DIR: /data
      MIHOMO_BINARY: /usr/local/bin/mihomo
    volumes:
      - ./data:/data
```

在 1Panel 的“容器 → 编排”中创建编排，工作目录选择项目目录，然后启动。也可以在终端中执行：

```bash
cd /opt/1panel/apps/local/sub2socks
docker compose up -d --build
```

### 4. 配置管理域名

在 1Panel 创建反向代理网站：

- 域名：例如 `proxy-admin.example.com`
- 代理地址：`http://127.0.0.1:3000`
- 启用 HTTPS
- 给整个网站添加 HTTP Basic Auth、IP 白名单，或仅允许通过 VPN 访问

不要在主机防火墙或云安全组中开放 TCP `3000`。

### 5. 开放 SOCKS5 端口

在以下两处按需开放 TCP `9999`：

1. 1Panel 主机防火墙
2. 云厂商安全组/防火墙

推荐将来源限制为自己的公网 IP；如果必须向全网开放，请使用强用户名和强密码，并留意异常连接。

## 配置项

可以通过环境变量配置：

| 变量 | 默认值 | 说明 |
| --- | --- | --- |
| `PORT` | `3000` | Web 管理界面/API 监听端口 |
| `SOCKS_PORT` | `9999` | 统一 SOCKS5 入口端口 |
| `RUST_PROXY_MANAGER_DB` | `proxy_manager.db` | SQLite 数据库路径 |
| `RUST_PROXY_MANAGER_DATA_DIR` | `data` | Mihomo 配置和运行数据目录 |
| `MIHOMO_BINARY` | 自动查找 | Mihomo 二进制路径 |
| `MIHOMO_EMBED_PATH` | 未设置 | 构建时嵌入 Mihomo 的路径 |

可复制 `.env.example` 作为环境变量参考：

```bash
cp .env.example .env
```

## 数据持久化与备份

默认 Docker Compose 使用名为 `data` 的 Docker Volume，保存：

- SQLite 数据库：`/data/proxy_manager.db`
- 生成的 Mihomo 配置：`/data/mihomo.yaml`

查看卷信息：

```bash
docker volume inspect sub2socks_data
```

如果使用 1Panel 推荐配置中的 `./data:/data`，数据位于项目目录下的 `data/`。

备份 SQLite 时建议先停止服务：

```bash
docker compose stop
tar -czf sub2socks-backup-$(date +%F).tar.gz data/
docker compose start
```

使用 Docker 命名卷时，请通过临时容器或 1Panel 的卷备份功能完成备份。

## 更新与卸载

更新代码并重建：

```bash
git pull
docker compose up -d --build
```

查看更新后的日志：

```bash
docker compose logs -f --tail=100
```

停止并删除容器：

```bash
docker compose down
```

保留数据时不要添加 `-v`。以下命令会同时删除 Compose 管理的数据卷，执行前务必备份：

```bash
docker compose down -v
```

## 常见问题

### 管理界面无法访问

检查容器和本机接口：

```bash
docker compose ps
docker compose logs --tail=200
curl http://127.0.0.1:3000/api/status
```

如果管理端口只绑定到 `127.0.0.1`，必须通过宿主机本地访问或配置反向代理。

### SOCKS5 连接被拒绝

依次检查：

1. Mihomo 是否显示为运行中
2. SOCKS 账号是否启用
3. 账号绑定的节点是否启用
4. 用户名和密码是否正确
5. TCP `9999` 是否已在主机防火墙和云安全组开放
6. 客户端是否支持 SOCKS5 用户名/密码认证

查看日志：

```bash
docker compose logs --tail=200
```

### Mihomo 无法启动

确认二进制存在且与系统架构匹配：

```bash
docker exec rust-proxy-manager /usr/local/bin/mihomo -v
```

从源码运行时，确认 `MIHOMO_BINARY` 指向一个存在且可执行的文件。

### 添加订阅失败

可以在服务器或容器内检查订阅地址是否可达。订阅 URL 通常包含敏感令牌，不要将完整 URL 粘贴到公开 Issue、日志截图或聊天记录中。

## 安全说明

当前版本存在以下已知限制：

- 初始化管理员账号后，管理 API **仍未实现认证和授权**
- 管理员密码当前以明文存入 SQLite
- SOCKS 账号列表 API 和编辑界面会处理明文密码
- 管理 API 可以启动/停止 Mihomo，并修改或删除订阅、节点和账号
- 订阅 URL 本身可能包含访问令牌，应按密码对待

部署时至少采取以下措施：

1. 将 Web 管理端口绑定为 `127.0.0.1:3000:3000`
2. 通过 HTTPS 反向代理访问管理界面
3. 在反向代理层启用 Basic Auth、IP 白名单或 VPN 访问控制
4. 不在公网开放 TCP `3000`
5. 为每个 SOCKS 账号使用不同的强密码
6. 尽量限制 TCP `9999` 的来源 IP
7. 限制数据库和备份文件的读取权限

在应用内鉴权、密码哈希和敏感字段脱敏完成前，不建议将管理后台直接用于公开生产环境。

## 开发

```bash
cargo fmt --check
cargo test
cargo run
```

项目主要技术栈：

- Rust
- Axum
- Tokio
- SQLite / rusqlite
- Mihomo
- 原生 HTML、CSS 和 JavaScript

提交代码前，请确保格式检查和测试通过，并实际启动应用验证受影响的用户流程。

## 贡献

欢迎提交 Issue 和 Pull Request。为了便于定位问题，请在反馈中包含：

- 操作系统和 CPU 架构
- Docker 与 Docker Compose 版本
- 应用日志中的相关错误
- 可复现步骤
- 使用的订阅格式或协议类型

请务必移除订阅 URL、节点凭据、SOCKS 密码、服务器 IP 等敏感信息。

## 路线图

- [ ] 管理端登录、会话认证与 API 授权
- [ ] 管理员和 SOCKS 密码安全存储
- [ ] API 敏感字段脱敏
- [ ] ARM64 镜像与多架构构建
- [ ] 更完整的订阅协议兼容性
- [ ] Mihomo 配置热更新与运行状态监控
- [ ] 自动化发布与版本升级文档

## 许可证

本项目基于 [MIT License](LICENSE) 开源。你可以自由使用、复制、修改、合并、发布和分发本项目，但需要保留原始版权声明和许可证文本。

## 致谢

- [Mihomo](https://github.com/MetaCubeX/mihomo) 提供代理核心能力
- Rust 与开源生态中的所有依赖项目
