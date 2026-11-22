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

这四条里最值钱的是第一条：**`keep_recovery` 在跨大版本升级里真的挡住了原厂 recovery**，
说明「装整包到 A 槽 + 保留 recovery」这条路在 4.0.4.0 上仍然成立。

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

- **看**：4.0.4.0 完整包的 URL、包名、字节数、sha256；从包里取出的 `recovery` 的 sha256。
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

4.0.4.0 完整包的 **URL**（你装的时候用的那个）、文件名、字节数、sha256。
拿到 URL 后我用 `skyward-pack stock --out` 只按 range 取 `recovery` 一段，
把它的 sha256 写回 `[stock]`，不必下 9 GB。

## 6. 判据（什么情况才需要改代码）

| 取证结果 | 结论 |
| --- | --- |
| 七个 `.ko` 都在、`flatten` 还在、HAL 八条路径都在、weaver 能解 | **零代码改动**，本轮只补文档与基线（基线可延后） |
| 某个 `.ko` 换了目录/名字，但仍在四个来源内 | 改 `devices/bixi.toml` 的 `load` / `sources`，重出、重验 |
| `system_dlkm` 不再有 `flatten` | 改 `skyward-modules/src/lib.rs` 的路径表（**这是代码**），配一条单测 |
| HAL 二进制换目录（如 `qwesd` 搬进 `/vendor/bin/hw/`） | 改 `[[service]].path` 与其 `libraries` 顺序 |
| weaver 读不到槽位 / protector 布局变了 | 停，先备份 `/data`，再单独啃 `credential.rs` |
