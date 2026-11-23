# XM KEEL · 变更记录

> 机型：小米 MIX Flip 2（`bixi`，SM8750 / `sun`，Android 17，A/B + 动态分区，出包基线 `OS4.0.0.11.XOHCNXM`）
> 最初源自 [kirklin/skyward](https://github.com/kirklin/skyward)（GPL-3.0），现由 XM KEEL 独立维护。
> 版本号自 1.0.0 起按 XM KEEL 自己的编号排，不再接上游或旧分支的号；1.0.13 及以前的记录在 [CHANGELOG-bixi.md](CHANGELOG-bixi.md)。

## 1.0.0

XM KEEL 的第一版，也是它从 skyward 这条线上分出来的一版。功能没有新东西：这一版做的全是名字——仓库、品牌、crate、机内路径——以及让已经在跑的机器升级上来时，什么都不丢。

**独立成仓。** 项目搬到 [XQAB/XM-KEEL](https://github.com/XQAB/XM-KEEL)，不再是 `kirklin/skyward` 的 fork，也不带上游关系：`.github/workflows/upstream-sync.yml` 删了，指向上游的 remote 删了，README 里那句「fork of kirklin/skyward」改成「最初源自 kirklin/skyward（GPL-3.0），现由 XM KEEL 独立维护」。LICENSE 与原作者署名原样保留——GPL 衍生作品必须留。

**品牌层。** 界面、文档、发布标题、更新检查、ABOUT 页里的版本串（`XM KEEL 1.0.0`）全部换成 XM KEEL。发布资产名仍以 `skyward-` 开头：1.0.13 的更新器只认这个前缀，改了名字等于让还在用旧版的手机永远看不到新版。

**内部 crate。** `crates/skyward-*` 那 21 个目录改成 `keel-*`，crate 名 `skyward_*` → `keel_*`，所有 `use skyward_*` 引用跟着改，二进制名改成 `keel`。UI 内部的标识符（`SkywardStage`、`skyward_job` 这类）不动，它们不是 crate 名。

**机内路径，带迁移。** `/metadata/skyward` → `/metadata/keel`（`credential`、`credential.new`、`settings.json`、`systems.toml`）；`/skyward/run`（日志、截图、supplicant）、`/skyward/persist`、`/skyward/park`、`/skyward/wlan`、命令行的 `device.toml` 与两个字体默认值 → `/keel/...`；镜像内 `keel/device.toml`、`keel/system-init`、`keel/MiSansVF.ttf`、`keel/KirkMono-*`，以及 `system/etc/init/keel-mtp.rc`。

开机时，`/metadata/skyward` 还在而 `/metadata/keel` 不在，就先把它整个搬过去，之后才读设置和密码。**已经装过的机器升级上来，密码照旧能解、/data 照旧能开**，不用重新输。旧镜像里的 `skyward/device.toml` 也仍然认得——认不出它，`keep` 会把装在槽里的老镜像当外来镜像覆盖掉。搬不动时只写一行 kmsg，下次开机接着搬。

**没跟着改的。** `/data/skyward/systems`、第二系统 `init_boot` 里的 `skyward/system.toml` 与 `skyward/map`：这些名字同时记在 `systems.toml` 和每套启动记录里，改一半会让已装的第二系统起不来。`Skyward/{backups,partitions,logs}`、`Download/Skyward`、插件目录、`/data/misc/wifi/skyward` 是用户自己的文件，改了等于丢。

**这一版的号。** 从 1.0.0 开始，是因为项目换了名字、换了仓库，不接着 skyward 的 1.0.13 往下排。代价要说明：还装着 1.0.13 的手机看到 1.0.0，会判成「不比本机新」，不会自己升级——要上新版请按 [docs/ota-survival.md](docs/ota-survival.md) 手动刷一次，之后就按新编号走。

CI：三个提交（品牌 → crate → 路径+迁移）各自 Build 与 Preview 双绿；真机升级与迁移验收待做。
