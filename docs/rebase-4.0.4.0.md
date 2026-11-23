# 从 OS4.0.0.11 到 OS4.0.4.0：需不需要重适配，以及检查清单

> 机型 bixi（MIX Flip 2）。2026-11-22 起，真机已从 `OS4.0.0.11.XOHCNXM` 升到
> `OS4.0.4.0.XOHCNXM`（Android 17 / SDK 37），A 槽 4.0.4.0 + KernelSU 3.3.1，
> 两槽 recovery 仍是 v1.0.13（`3676c676…54aa06`）。本文回答「旧 recovery 要不要跟着改」，
> 并按 [`ota-rebase.md`](ota-rebase.md) 的顺序给出取证清单。**本文只给结论与清单，不含代码改动。**

## 0. 结论（三句）

1. **运行时不用重做。** v1.0.13 在 4.0.4.0 上已经起得来（实测进得去、`boot_completed=1`），
   因为镜像本身是 **ramdisk-only**（v1.0.13 的 `verify.txt` 第 4 行：`kernel size: no kernel: the recovery is the ramdisk alone`），
   内核来自 `boot` 分区（现在就是 4.0.4.0 的），模块与 HAL 二进制一律按固定挂载点
   从手机**当前**的 `vendor_dlkm` / `vendor` / `odm` / `system` / `system_dlkm` 现取。
   这是「大版本换了、recovery 却还能跑」的原因，不是它被适配过。
2. **打包基线要跟着换，但可以推迟到「换底座」那一次一起做。** `devices/bixi.toml` 的
   `[stock]` 三行（`version` / `package` / `recovery`）决定：出包从哪个完整包里拆 stock recovery、
   资产名带哪个版本、`verify-release.py` 核对 `prop.default` 是否同名。只要不换，构建仍可重复
   （旧包还在小米服务器上），代价只有「资产名与 About 页写着 4.0.0.11，机器跑的是 4.0.4.0」。
3. **代码要不要改，只取决于第 3 节取证。** 若 4.0.4.0 没搬动我们按名字取的东西，本轮**零代码改动**；
   真搬了，改的也只是 `devices/bixi.toml` 的名单/路径（一个例外见 3.3：`system_dlkm` 的
   `flatten` 视图变了要改 `skyward-modules` 的路径表，那才算代码）。

## 1. 两种「重适配」，别混为一谈

| | 含义 | 代价 | 本轮做不做 |
| --- | --- | --- | --- |
| **A. 跟随基线**（只改 `[stock]`） | 出包仍用 **4.0.4.0 的 stock recovery** 做底座，资产名改成 4.0.4.0 | 取包内 `recovery` 一段（~100 MB，range 读，不必下 9 GB）+ 重出一次 + 真机重验六条链 | 建议**延后**到 B 一起，或等 A 的取证暴露问题再做 |
| **B. 换底座**（A + 重验） | ramdisk 里 `init`、`prop.default`、`init.rc` 换成 4.0.4.0 的 | 与 A 同一次动作，加上 §4 六条链全部重验 | 下一版跟随，不在本次真机升级里做 |

现在这台机跑的是「**4.0.0.11 的 ramdisk 底座 + 4.0.4.0 的分区与内核**」这种混合态。
它能不能算数，全靠第 3 节把「按名字取」的每一样东西点一遍。

## 2. 已定的事实（真机读回，不用再验）

- `recovery_a` / `recovery_b` = `3676c676ebdd82c68fc4fe914d65a317426e518d090b537eaae4a8522354aa06`（v1.0.13 保留，未被原厂覆盖）。
- `init_boot_a` = `7c1453fa…`（KernelSU 补丁）；`boot_a`/`vbmeta_a`/`dtbo_a`/`vendor_boot_a` 均为 4.0.4.0。
- `/metadata/skyward/settings.json` 完好：`keep_recovery=true`、`remember_credential=true`。
- recovery 可进，v1.0.13 起得来。

第一条要改读法（2026-11-22 修正）。**这次升级不是走 rec 装整包完成的**：进 Skyward rec
点安装 4.0.4.0 完整包没有成功（另案，见 §7），最后是别的办法进的系统。所以
`recovery_a/b` 仍是 `3676c676…` 只说明「没有任何东西写过这两个分区」，**推不出**
`keep_recovery` 的 hold → 装 → reflash 这条链在 4.0.4.0 上验证过；要验它，得等一次
rec 内装整包真正走通。

### 2.1 4.0.4.0 完整包本身的读数（从包内元数据读出）

- 文件 `bixi-ota_full-OS4.0.4.0.XOHCNXM-user-17.0-a6ba7e3e2c.zip`，`9432900450` 字节，
  `sha256 = 4f634159e94c9b1cd9d646be93804cdc70f235559826784b2745a2058f898ca5`；
  镜像地址回 200 且 `Content-Length` 与本地同值（`…/OS4.0.4.0.XOHCNXM/` 那一个文件名）。
- `ota-type=AB`、`pre-device=bixi`、**没有 `pre-build`/`pre-build-incremental`**（是完整包）、
  `ota-required-cache=0`、无 `ota-wipe`、`post-sdk-level=37`、
  `post-security-patch-level=2026-10-01`、`post-timestamp=1794506917`。
- `post-build-incremental=17OS4.0.261113.004012506.QCPECN.S`，**不等于**标签 `OS4.0.4.0.XOHCNXM`。
  安装确认页与引擎日志显示的是这个长串（`Update::version()` 取的就是它），属正常；
  About 页读 `ro.mi.os.version.incremental`（`devices/bixi.toml:7`），仍是 `OS4.0.4.0.XOHCNXM`。
- payload 在 zip 内起点 `4942`、`9432891836` 字节（与包内 `ota-property-files` 完全一致），Stored；
  manifest 含 `boot`/`init_boot`/`vbmeta`/`dtbo`/`vendor_boot`/**`recovery`**/`system`/`vendor`/
  `product`/`odm`/`system_ext`/`system_dlkm`/`mi_ext` 以及 `super` 的动态分区元数据。
  → **一次成功的整包安装一定会把 recovery_a/b 写成原厂**，`keep_recovery` 的回写必须真跑过才算数。

## 3. 必须取证才知道的（按风险从高到低）

每条都写：看什么、变了会怎样、我们仓里对应哪一行。命令见 §5。

### 3.1 解密链路（最高风险）

- **看**：密码解锁能不能打开 `/data`（`Bound::Weaver` 路径）；`/data/misc/vold` 的 protector
  文件是否还是同布局（`user.rs` 的 `WEAVER_SLOT_LAYOUT = 1`）；`skyward --weaver` 能不能读出槽位数。
- **变了会怎样**：密码对但打不开，或者直接报 weaver 槽读不到——这是唯一会让人「以为 recovery 坏了」的一条。
- **对应**：`devices/bixi.toml` 的 weaver / keymint / ssgtzd / qseecomd 四条服务（`:73-165`），
  `crates/skyward/src/credential.rs` 的 weaver 绑定。
- **注意**：4.0.4.0 的 `/data` 是它自己迁移过的，**先确认 data 里没有未完成的 OTA 快照**再去解；不行就先备份。

### 3.2 TEE 服务链是否注册

- **看**：`qseecomd` → `vendor.qti.hardware.qseecom@1.0-service` → `qwesd` → `ssgtzd` →
   `keymint` → `secure_element` → `weaver` → `se_omapi` 八条是否都起来、`ready_service` 是否注册。
- **变了会怎样**：keymint 拿不到密钥 → 解密/备份都受影响；`vendor_ssgtzd` 组名变了 → ssgtzd 起不来。
- **对应**：`devices/bixi.toml` `[[service]]` 八段；`/vendor/etc/group` 里的 `vendor_ssgtzd`（2912）；
  `/system/etc/vintf/weaver-nxp.xml` 是否还在。

### 3.3 模块与 dlkm 路径

- **看**：`synaptics_tcm2`、`xiaomi_touch`、`mitee_dlkm`、`nxp-nci`、`goodix_core`（副屏按需 insmod）、
  `rfkill`、`qca_cld3_peach_v2` 七个 `.ko` 在不在、在哪个目录；`modules.dep` 有没有 `xiaomi_touch → synaptics_tcm2` 的依赖边；
  `/system_dlkm/flatten/lib/modules` 这个 **flatten 视图**还在不在（`rfkill` 从这里取）。
- **变了会怎样**：内屏触摸、副屏触摸、WLAN 依次失效；`system_dlkm` 若不再有 `flatten`，
  `rfkill` 找不到 → `wlan0` 起不来（这一条是**代码里的路径表**，不是配置）。
- **对应**：`devices/bixi.toml:24-42`（`sources` 与 `load`）、`crates/skyward-modules/src/lib.rs:12-19`
  （`vendor_boot`→`/lib/modules`、`vendor_dlkm`→`/vendor_dlkm/lib/modules`、
  `vendor`→`/vendor/lib/modules`、`system_dlkm`→`/system_dlkm/flatten/lib/modules`）。

### 3.4 触摸与显示参数

- **看**：`/sys/class/touch/touch_dev/enable_touch_raw` 在不在（`[touch].writes` 写它）；
  `/proc/bus/input/devices` 里 `synaptics_tcm_touch`(event7) 与 `goodix_ts`(event8) 的 ABS 范围
  （内屏 19583/46591 = 1224×2912×16，副屏 1208×1392）；`ro.miui.density.secondaryppi` 还是不是 460；
  `/sys/class/qcom-haptics/lra_frequency_hz` 还在不在。
- **变了会怎样**：触摸点落在错位置（换算了但范围变了）、UI 布局密度不对、马达不震。
- **对应**：`devices/bixi.toml:9-22`（display，值写死 1224×2912 / 460 / 134）、`:47-48`（touch writes）、
  `:245-248`（haptics）。

### 3.5 WLAN 固件与模块名

- **看**：`/vendor/etc/wifi/peach_v2/WCNSS_qcom_cfg.ini` 路径是否还在（**必须**是
  `/etc/firmware/wlan/qca_cld/peach_v2/` 这个加载名）；`/mnt/vendor/persist/wlan/wlan_mac.bin` 是否还在；
  模块名还是 `qca_cld3_peach_v2` 而不是 `peach_v3`。
- **变了会怎样**：`Failed to probe host driver, err=-1`，`wlan0` 永不出现，「自动加入」失效。
- **对应**：`devices/bixi.toml:210-227`。

### 3.6 分区表与镜像

- **看**：`lpdump` 的分区名/尺寸；`recovery_a/b` 的尺寸是否仍 ≥ 104857600（我们的镜像正好这么大）；
  `super`/`userdata`/`metadata` 的 extent 有没有挪。
- **变了会怎样**：`recovery` 若被缩小到 100 MB 以下，dd 写不进去；双系统按 extent 读写 super 会错位。
- **对应**：`skyward-vab` 的按名取分区、`skyward-systems` 的 super extent。

### 3.7 两槽一致性

- **看**：`boot_a`/`init_boot_a` 与系统版本是否同版（这次是 KSU 补丁过的 `init_boot_a`，属已知）；
  B 槽那套是 4.0.0.11 的旧值。
- **变了会怎样**：从 B 槽起机是旧系统 + 旧 recovery，属预期；但若哪天切槽，别把 A 槽的 4.0.4.0 与旧 init_boot 混起来（`ota-rebase.md` §7 的老坑）。

### 3.8 打包基线（见 §1 A）

- **看**：包地址与字节数已核（§2.1）；**只剩**从包里取出的 `recovery` 的 sha256。
- **变了会怎样**：不改也能构建（旧包仍在），但资产名/About 页/`verify-release.py` 的名字核对都与机器现状脱节。
- **对应**：`devices/bixi.toml:239-242`、`.github/stock/bixi.txt` 三行。

## 4. 可以不动（附理由）

- **UI / A/B / AVB / 备份 / 擦除的逻辑**：都不认系统版本，只认分区名与属性，分区名没变就不用动。
- **`[modules].sources` 的四个来源名**（`vendor_boot`/`vendor_dlkm`/`vendor`/`system_dlkm`）：
  这是 AOSP 的约定，不是小米的自定义，4.0.4.0 不会改这四个分区的作用。
- **`[backup] efs` 名单**（`modemst1/2`、`fsg`、`fsc`、`persist`）：分区名由 GPT 定，跨小版本不动。
- **`[update] server`**：仍是小米更新接口。
- **双系统 guard**（`skyward-systems/src/guard.rs`）：比的是安装时存下的 SHA-256，不认版本号；
  只要没装第二系统（`state.system` 为空），这条根本不参与。
- **`[stock].recovery` 的 4.0.0.11 值**：只要不换底座，它仍然是对的（旧包还在）。

## 5. 我要抓的数据（一次抓完，别逐条往返）

### Android 侧（KSU root，`adb shell su -c '<cmd>'`）

```sh
getprop ro.mi.os.version.incremental; getprop ro.build.version.incremental
getprop ro.build.version.sdk; getprop ro.miui.density.secondaryppi
ls /vendor_dlkm/lib/modules | grep -E 'synaptics|xiaomi_touch|mitee|nxp|goodix|rfkill|qca_cld|modules.dep'
ls /vendor/lib/modules | grep -E 'synaptics|xiaomi_touch|mitee|nxp|goodix|rfkill|qca_cld'
ls -d /system_dlkm/* /system_dlkm/flatten/lib/modules 2>&1 | head -20
ls -l /vendor/bin/qseecomd /vendor/bin/qwesd /vendor/bin/ssgtzd \
      /vendor/bin/hw/vendor.qti.hardware.qseecom@1.0-service \
      /vendor/bin/hw/android.hardware.security.keymint-service-qti \
      /vendor/bin/hw/android.hardware.secure_element-service.qti \
      /odm/bin/hw/android.hardware.weaver-service.nxp-qti /system/bin/se_omapi
ls -l /vendor/etc/wifi/peach_v2/WCNSS_qcom_cfg.ini /odm/firmware/goodix_firmware_bixi.bin \
      /odm/lib64/libtouchreport.so /system/etc/vintf/weaver-nxp.xml \
      /mnt/system/system/lib64/libsysutils.so /mnt/system/system/lib64/libcap.so
grep -c vendor_ssgtzd /vendor/etc/group
ls -l /sys/class/touch/touch_dev/enable_touch_raw /sys/class/qcom-haptics/lra_frequency_hz
sha256sum /dev/block/by-name/boot_a /dev/block/by-name/init_boot_a \
          /dev/block/by-name/vendor_boot_a /dev/block/by-name/dtbo_a \
          /dev/block/by-name/vbmeta_a /dev/block/by-name/vbmeta_system_a
```

### recovery 侧（进 rec 后，`adb shell` 直接可用）

```sh
head -40 /skyward/run/recovery.log
cat /proc/mounts | grep -E 'vendor_dlkm|system_dlkm|/vendor |/odm |/mnt/system'
cat /proc/modules | grep -E 'synaptics|xiaomi_touch|mitee|nxp|goodix|rfkill|qca_cld'
service list | grep -E 'keymint|weaver|secure_element|omapi|qseecom|bootcontrol'
cat /proc/bus/input/devices | grep -B1 -A4 -E 'goodix|synaptics'
lpdump --slot 0 | head -60
ip addr show wlan0; dmesg | grep -iE 'cnss|peach|wlan0' | tail -20
dmesg | grep -E 'partitions not mounted|devices not brought up|mounted'
```

### 只在真机上做、没法用命令替代的三项

1. 密码解锁 → 能不能进主页并看到 `/data` 内容（3.1）；
2. 合盖 → 熄屏 → 点亮 → 点一下（副屏触摸，v1.0.13 的验收项，在 4.0.4.0 上重做一次）；
3. 设置页里「Skyward 更新」那一页读到的机器版本（看 About 页 built on / running 两行差多少）。

### 打包侧

包地址、字节数、sha256 都已到手（§2.1），不用再给。剩下的是从包里拆出 `recovery`
那一段并算它的 sha256（`skyward-pack stock --out` 只按 range 取这一段，不必下 9 GB），
填回 `[stock].recovery`；`[stock].package` 直接填 §2.1 那条镜像地址。

## 6. 判据（什么情况才需要改代码）

| 取证结果 | 结论 |
| --- | --- |
| 七个 `.ko` 都在、`flatten` 还在、HAL 八条路径都在、weaver 能解 | **零代码改动**，本轮只补文档与基线（基线可延后） |
| 某个 `.ko` 换了目录/名字，但仍在四个来源内 | 改 `devices/bixi.toml` 的 `load` / `sources`，重出、重验 |
| `system_dlkm` 不再有 `flatten` | 改 `skyward-modules/src/lib.rs` 的路径表（**这是代码**），配一条单测 |
| HAL 二进制换目录（如 `qwesd` 搬进 `/vendor/bin/hw/`） | 改 `[[service]].path` 与其 `libraries` 顺序 |
| weaver 读不到槽位 / protector 布局变了 | 停，先备份 `/data`，再单独啃 `credential.rs` |

## 7. rec 内装 4.0.4.0 完整包：装完了，是 A 槽第一次启动没起来（另案，未结案）

**前提已纠正（2026-11-23）**：失败不在安装流程里——安装流程走完了。11-22 那次装包的
`/skyward/run/recovery.log` 自己留下三行：

    install: left for the next start: snapshots Snapshotted, merge outstanding true
    install: this recovery written back to [A]: Ok(())
    install: Installed { slot: A, wipe: false }

第三行只在 update_engine **退出码 0** 之后才打（`run.rs:629`）。所以：A 槽写进去了、recovery
写回了 `[A]`（顺带说明 1.0.13 的 `make_writable`/`BLKROSET` 在 4.0.4.0 上真管用）、没有 wipe。
没成的是**重启进 A 槽那一步：卡开机 logo**。

现场：`/metadata/ota/state` 74 字节；`/metadata/ota/snapshots/` 下有 9 个分区的快照
（`system_a`/`vendor_a`/`product_a`/`odm_a`/`system_ext_a`/`system_dlkm_a`/`vendor_dlkm_a`/
`mi_ext_a`/`mi_product_a`）；`merge outstanding true`。**这是一次走了 Virtual A/B 快照的安装，
第一次启动必须 merge；卡 logo 就发生在 merge 这一段。**

### 7.1 包大小这一条：只作上限记录，不是病因

四条代码事实，都可复核：

1. **块大小随包长自适应**。`fuse::file_block` 把块放大到 `size / MAX_BLOCKS` 的上一个 2 的幂：
   `9432900450 / 262144 = 35,980` → `64 KiB`，于是 `143,935` 块，上限 `adb::MAX_BLOCKS = 262,144`
   → **还有一倍余量**。（只有块停在 4 KiB 时才会 `2,302,900` 块被 `Offer::check` 拒。）
2. **payload 起点是对的**。我们从 zip 的 local header 算得 `4,942`，包内 `metadata` 的
   `ota-property-files` 也写 `4,942`；zip64 的 26 字节 extra 已算进偏移。
3. **全链路 u64**：`Offer`、`--offset`、`--headers` 的 `FILE_SIZE=9432891836`，没有 4 GiB/2 GiB 截断点。
4. **不解包到磁盘**：`/sideload/package.zip` 是 FUSE 现取（`skyward-install/src/fuse.rs`），
   不存在「临时空间不够」这一类坑。

安装既然走完，这四条就只是「这个包在我们这儿不会撞上限」的记录，与本次失败无关。

### 7.2 首启为什么没起来（待证，收敛成两条）

与「安装阶段」有关的那几条可能性，随 `Installed { slot: A }` 一起作废。剩下的两条，判据都写在
§7.5 —— 因为原来打算用的内核侧证据（pstore）在这台机上已经确认拿不到（§7.4 第 2 条）。

| # | 机制 | 判据 |
| --- | --- | --- |
| 1 | **merge 段本身卡住**：COW 在重启前没落盘、或 COW 在首启的系统视图里打不开、或 `snapuserd` 没起来。这三条现在**无法互相区分**（都只表现成 merge 不动），要分开得靠内核日志，而这台机没有 | 卡住后进 rec：`/metadata/ota/state` 仍为 `Snapshotted`/`Merging`，`snapshots/` 目录还在 |
| 2 | **merge 走完了，卡在更后面**：A 槽镜像自己起不来（vbmeta/AVB 与写入内容不合、`init_boot` 被换成原厂后与残留的 KSU 冲突等） | 卡住后进 rec：state 已是 `None`、`snapshots/` 清空，但 `/data/misc/bootstat` 里那一次启动没有 `boot_completed` |

### 7.3 一个能二选一的判别实验（下次复现先做这个）

| | 路径 | 引擎行为 | 预期 |
| --- | --- | --- | --- |
| A | 内部存储（本次路径） | `/data` 开着 → 建快照 → 首启要 merge | 仍卡 logo（则病因在 §7.2 第 1 条） |
| B | 同一份包，`adb sideload` | `/data` 关着 → 只能 `Will overwrite existing partitions`（`Log::Overwriting`），首启**不需要 merge** | 能进系统，就证明病因是「快照 → 首启 merge」，与包无关 |

可执行的步骤、前置、判据表和记录表在 **`docs/exp-404-sideload.md`**，照抄即可跑。主判据只有
一行：装完后 `recovery.log` 的 `left for the next start` 从 `Snapshotted, merge outstanding
true` 变成 `snapshots None, merge outstanding false`。

B 的代价：直接覆盖写 A 槽，失败时 A 槽起不来（已有先例，重刷可回）；`init_boot_a` 被包内原厂
镜像覆盖，**A 槽的 KernelSU 会没**，实验后要重刷补丁。

### 7.4 现场已核（2026-11-23，手机插回后实测）

1. **`/metadata/ota` 干净**：`state` 0 字节、`snapshots/` 空、`update_engine/` 子目录不存在。
   ⇒ 开头担心的 `Refusal::Pending` 不存在，下次装包不会被「更新已写入，等待合并」当场拒；
   也吻合「失败的那次启动让 VAB 状态走了标准作废流程」。
2. **`/sys/fs/pstore/` 是空目录**（`total 0`，只有 `.` 和 `..`）：这台机没有任何 pstore 后端在
   留东西，**卡 logo 那次的内核日志已经拿不到了**，原来那条路线作废。要确认是不是内核压根没
   注册：`dmesg | grep -iE 'pstore|ramoops'`、`ls /sys/fs/pstore`。
3. **A 槽现在的 4.0.4.0 是好的**：`sys.boot_completed=1`、`ro.boot.slot_suffix=_a`、
   `bootreason=reboot,userrequested`、`veritymode=enforcing`（原样记录）。这套内容来自「别的办法」，
   不是 rec 装的那一份。
4. **11-22 那次安装确实写到了 `init_boot_a`**：装完当场读到原厂 `6e636c81…`，现在是 `7c1453fa…`
   的 KSU 补丁。⇒ 写入路径（`image::flash` + `make_writable`）在 4.0.4.0 上是对的，病因不在
   「写错内容」，而在「这套内容首启能不能起来」。

分区读回（11-23，sha256 前 16 位）：

| | 槽 A | 槽 B |
| --- | --- | --- |
| boot | `d2d363b2619a9083` | `55af7f87a00d3496` |
| init_boot | `7c1453fa4641fab6`（KSU） | `1cfa51eaa7de6888` |
| recovery | `3676c676ebdd82c6`（v1.0.13） | `3676c676ebdd82c6`（v1.0.13） |
| vbmeta | `dd457f4ca98d9081` | `e6acbd65679085bf` |
| dtbo | `f690924dda49e37c` | `6e60d8d4b952d732` |
| vendor_boot | `42fd42055df23e36` | `0d3b89fbd9fbbf1a` |

### 7.5 取证（pstore 不可用了，改这三处 + 两个前置动作）

先把结论说清：卡 logo 那次的「内核那一侧」证据，这台机**拿不到**（pstore 空、生产版没有 adbd）。
所以下一次复现的价值全在两个前置动作，其次是卡住后进 rec 看 state，再其次是框架侧三处。

**前置（这次没做，下次必须做）**

```sh
# 1) 装完、重启之前，把 tmpfs 里的日志取走（重启即失）
adb pull /skyward/run/recovery.log       ./recovery.log
adb pull /skyward/run/update_engine.log  ./update_engine.log
adb pull /skyward/run/install-script.log ./install-script.log 2>/dev/null
# 2) 重启之前在 rec 里读回刚写进去的 A 槽内容（证明写的是什么）
for p in boot_a init_boot_a vbmeta_a dtbo_a vendor_boot_a recovery_a ; do
  printf '%-14s ' $p ; sha256sum /dev/block/by-name/$p | cut -c1-16
done
```

**卡住之后，不进系统、先进 rec —— 这是唯一还能拿的一手判据**

```sh
cat /metadata/ota/state | od -An -tx1 | head -4
ls -l /metadata/ota/snapshots
```

- state 仍为 `Snapshotted`(2)/`Merging`(3)、`snapshots/` 还在 ⇒ merge 没走完（§7.2 第 1 条）；
- state 已是 `None`(0)、`snapshots/` 清空 ⇒ merge 走完了，卡在更后面（§7.2 第 2 条）。

**框架侧三处（现在就能抓，回答的是「后来那次为什么能起来」）**

1. `/data/system/dropbox/`（要 root：`adb shell "su -c 'ls -l /data/system/dropbox'"`）：
   找 `last_kmsg` / `system_boot` / `system_restart` / `watchdog` 命名的条目。
   `SYSTEM_LAST_KMSG` 是 system_server 启动时采的上一次内核日志，`SYSTEM_BOOT` 说明那次确实到了
   framework。**只覆盖「到了 system_server 的启动」**，卡 logo 那次大概率没有。这些文件带 DropBox
   自己的头 + gzip，别直接 `zcat`（会报错），拷出来解析。
2. `/data/misc/bootstat/`：一事件一文件。看两件事：卡住那次有没有 `boot_completed`；
   `boot_reason_*` / `boot_subsystem_*` 系列里有没有异常码。同样只覆盖到 framework 的启动。
3. `adb shell 'logcat -b all -d | grep -iE "snapshot|merge|update_engine|snapuserd"'`：这是**当前
   boot** 的日志。若「别的办法」是系统内 OTA，会看到 `update_engine` 的痕迹；若是 fastboot 直接
   刷分区，一条都没有 —— **这本身就是「别的办法是哪一种」的证据**。

「别的办法」具体是哪一种（系统内 OTA / MiAssistant / fastboot 直刷 A 槽分区）仍在问。若是系统内
OTA，说明这台机、这个包的 merge 链本身是通的，差别只在「我们这条路径」。

