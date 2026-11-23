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

### 3.2 TEE 服务链是否注册 —— 4.0.4.0 实测（2026-11-23）

八条：`qseecomd` → `vendor.qti.hardware.qseecom@1.0-service` → `qwesd` → `ssgtzd` →
`keymint` → `secure_element` → `weaver` → `se_omapi`。**七条都在、路径没变**；
`se_omapi` 那条要改读法，见 §3.2.1。

- 服务定义全在 `/vendor/etc/init/*.rc`（**不是** `hw/` 子目录）：
  - `qseecomd.rc`：`class core`、`user root`、`group root drmrpc`、
    `socket notify-topology stream 660 system drmrpc`
  - `qwesd.rc`：`class late_start`、`user system`、`group system inet`、`disabled`
  - `ssgtzd.rc`：`class late_start`、`user system`、`group system oem_2912 wakelock`、
    `capabilities WAKE_ALARM BLOCK_SUSPEND`、`disabled`，两个 socket
    （`ssgtzd 0660 system oem_2912`、`ssgtzd_opener 0660 system system`）
  - `keymint`：`class early_hal`、`user system`、`group system drmrpc`
  - `gatekeeper`：`class early_hal`、`user system`、`group system`
  - `weaver_nxp`：`class hal`、`user vendor_nxp_weaver`、`group vendor_nxp_weaver`、
    `capabilities WAKE_ALARM`
  - `secure_element`：`class hal`、`user system`、`group nfc system`；
    实测 `/proc/2028/status`：`Uid: 1000`、`Gid: 1027`、`Groups: 1000`
- `oem_2912` 是 init 给 vendor 组自动加的别名，`/vendor/etc/group` 里只有
  `vendor_ssgtzd::2912:`，两者同一个组 —— toml 写 `vendor_ssgtzd` 不用改。
- `weaver` 实测 `/proc/2151/status`：`Uid: 2911 2911 2911 2911`、
  `Gid: 2911 2911 2911 2911`、`Groups:`（空）；`/vendor/etc/group` 有
  `vendor_nxp_weaver::2911:`，`/vendor/etc/passwd` 有
  `vendor_nxp_weaver::2911:2911::/:/bin/sh`。**与 toml 现值（`root`/`root`）不一致**，
  但先不改，理由见 §3.2.2。
- VINTF 描述的位置（旧版这一节写错了几处，以实测为准）：
  - keymint：`/vendor/etc/vintf/manifest/android.hardware.security.keymint-service-qti.xml`，
    版本 3，一个文件报四条：`IKeyMintDevice/default`、
    `IRemotelyProvisionedComponent/default`、`ISecureClock/default`、`ISharedSecret/default`
  - weaver：`/odm/etc/vintf/manifest/android.hardware.weaver-service.nxp.xml`，版本 2，
    `IWeaver/default` —— 旧版写的 `/system/etc/vintf/weaver-nxp.xml` **手机侧已经没有了**
    （那个文件是我们自己 `bixi_extra/` 里的，同路径但不同主人，见 §3.2.1 的读法）
  - gatekeeper：`/vendor/etc/vintf/manifest_sun.xml`（按 build 名存的主描述，不在
    `manifest/` 目录里），`IGatekeeper/default`、`target-level=202404`
  - secure_element：`/vendor/etc/vintf/manifest/android.hardware.secure_element.xml` 报
    `ISecureElement/SIM1`、`SIM2`；`/odm/etc/vintf/manifest/manifest_nxp_nfc_ese.xml` 报
    `ISecureElement/eSE1`。实例名是 **`eSE1`**；上游 `EMBEDDED = "eSE"` 用 `starts_with`
    匹配，`"eSE1".starts_with("eSE")` 为真，能对上，但别写成 `eSE`
  - omapi：`/vendor/etc/vintf/manifest/secure_element-service.xml`，`android.se.omapi` v1
  - boot：`/vendor/etc/vintf/manifest/boot-service.qti.xml`，`IBootControl/default`
- 1.0.13 没有的一条：`/odm/bin/hw/android.hardware.security.keymint-service.strongbox-nxp`
  （strongbox keymint + sharedsecret，`vendor.keymint-strongbox`）。
- 两个 socket 都在：`/dev/socket/notify-topology`（system:drmrpc）、`ssgtzd`
  （system:vendor_ssgtzd）、`ssgtzd_opener`（system:system）。
- `persist` 挂在 `/mnt/vendor/persist`（`/dev/block/sdf7`，ext4），与
  `[partitions] mounted = ["persist"]` 对得上。

#### 3.2.1 `se_omapi`：手机侧删了，我们侧没删

4.0.4.0 的 Android 侧已经没有 `/system/bin/se_omapi`（`find /system /system_ext /vendor
/odm -name 'se_omapi*'` 全空，只剩几个 permissions xml），现在由 `com.android.se`
（`/system/app/SecureElement/SecureElement.apk`，实测 pid 6819、用户 `secure_element`）
在 `android.se.omapi.ISecureElementService/default` 上服务。

但**我们跑的那个 se_omapi 不是手机 system 分区里的文件**：它是
`bixi_extra/system/bin/se_omapi`，出自 `[stock]` 指的 OS4.0.0.11 包
（`bixi_extra/SOURCES.md` 第 14 行），由 `skyward-pack --extra bixi_extra` 打进
recovery 自己的 `/system`。v1.0.13 的资产报告里就能看到它：

- `v1013_verify.txt:24` —— `ok extra file: system/bin/se_omapi, 139136 bytes`
- `:31-39` —— 它要的 `libbase` / `libbinder_ndk` / `libutils` / `libc++` 等逐条解析到

所以「4.0.4.0 删了这个文件 ⇒ recovery 里 se_omapi 没了 ⇒ weaver 起不来 ⇒ 密码解锁
已坏」这条链**不成立**。真正该问的是另一件事：这份 4.0.0.11 的二进制在 4.0.4.0 的
`libbinder_ndk` / `libutils` 上还能不能跑（recovery 里它从 `/mnt/system/system/lib64`
取库），以及 weaver → omapi → secure_element 这条 binder 链在 4.0.4.0 上是否仍通。
**这只能上机看，不能从「文件在不在」推。** 判据三条，都在 recovery 里：

1. `ls -l /system/bin/se_omapi` —— 文件在（139136 字节）
2. `service list | grep android.se.omapi` —— 服务注册上了
3. `/skyward/run/recovery.log` 里 se_omapi 那条有没有被 `started`；失败时它带的是
   加载器报错还是 binder 报错 —— 前者是 ABI 问题，后者才是服务问题

#### 3.2.2 weaver 的 uid：实测是 2911，但先不改

`vendor_nxp_weaver`（2911）是手机侧的实况；toml 现在写 `root`/`root`。改成 2911 之前
要先解决节点归属：手机侧那些 `/dev/teepriv0`、`/dev/xts*` 是它的 ueventd 交给 2911 的，
而 recovery 的 `bixi_extra/system/etc/ueventd.rc` 只给了 `/dev/nq-nci 0666 root root`，
其余 TEE 节点是内核默认的 `root:root`。**以 2911 去开就是被拒。** 要走这条，得同时用
`[owners]`（`main.rs` 的 `hand_over`，在模块加载之后跑）把这些节点交给
`vendor_nxp_weaver`；节点清单要实测 `ls -l /dev/teepriv* /dev/xts*` 才有，现在没有。
所以：**先留 root，等上机拿到节点清单再一起改**，别只改 uid。

### 3.3 模块与 dlkm 路径 —— 4.0.4.0 实测（2026-11-23）

七个 `.ko` 全在 `/vendor_dlkm/lib/modules`（398 条目），`panel_event_notifier.ko` 也在；
`/vendor/lib/modules` 是指向它的 symlink，同一份。`flatten` 还在，
`/system_dlkm/flatten/lib/modules` 有内容；`rfkill.ko` 两处都有 ——
`/system_dlkm/lib/modules/6.6.118-android15-8-g0908354dac1b-ab16143355-4k/kernel/net/rfkill/`
与 `/system_dlkm/flatten/lib/modules/`。内核 `6.6.118 … g-0908354dac1b`（8-20 那版）。

依赖边与旧记一致：

- `synaptics_tcm2.ko` → `xiaomi_touch.ko`、`panel_event_notifier.ko`
- `goodix_core.ko` → `synaptics_tcm2.ko`、`xiaomi_touch.ko`、`panel_event_notifier.ko`
- `nxp-nci.ko` → `smcinvoke_dlkm.ko`、`mem_object.ko`、`si_core_module.ko`、
  `qseecom_proxy.ko`、`xiaomi_touch.ko`、`panel_event_notifier.ko`

所以这一条**没有变化，不用改配置**。对应 `devices/bixi.toml:24-42`（`sources` 与 `load`）、
`crates/skyward-modules/src/lib.rs:12-19`。

### 3.4 触摸与显示参数 —— 4.0.4.0 实测（2026-11-23）

- `libtouchreport.so` **在 4.0.4.0 里已经不存在**（`/odm/lib64/` 只剩
  `sensors.touch.detect.so` 这类）。触摸算法换成了
  `/vendor/lib64/vendor.xiaomi.hw.touchfeature-V1-ndk.so`，服务声明在
  `/odm/etc/vintf/manifest/vendor.xiaomi.hw.touchfeature-service.xml`。
  这对我们**没有行为影响**（recovery 本来就不跑小米的算法，靠
  `[touch].writes` 让控制器自己报坐标），只是 `devices/bixi.toml` 里那句注释
  指错了路径，已改。
- `/sys/module/xiaomi_touch/parameters/` **不存在** —— xiaomi_touch 没有模块参数目录，
  别照旧文档去写它。
- 其余（`/sys/class/touch/touch_dev/enable_touch_raw`、两块屏的 ABS 范围、
  `ro.miui.density.secondaryppi=460`、`/sys/class/qcom-haptics/lra_frequency_hz`）
  这一轮没有实测读数，**按旧值保留**。

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
没成的是**重启进 A 槽那一步：起来了又倒，倒了 5 次之后被回滚**（见下面的证据链）。

现场：`/metadata/ota/state` 74 字节；`/metadata/ota/snapshots/` 下有 9 个分区的快照
（`system_a`/`vendor_a`/`product_a`/`odm_a`/`system_ext_a`/`system_dlkm_a`/`vendor_dlkm_a`/
`mi_ext_a`/`mi_product_a`）；`merge outstanding true`。**这是一次走了 Virtual A/B 快照的安装，
第一次启动必须 merge。**

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

### 7.2 首启为什么没起来（定案后的排序）

**定案（2026-11-23）**：「别的办法」是系统 OTA 一类。这一条把三件事同时定了：

1. **merge 链在这台机、这个包上是通的**。系统 OTA 一样是 `/data` 开着、引擎建 COW、写下
   `Snapshotted` + `merge outstanding true`，然后首启 merge 走完、进了系统（现在
   `sys.boot_completed=1`、`veritymode=enforcing`）。
2. **merge 的执行者是同一套**：首启的 `init` + `snapuserd`。它那一次合并完了，所以「执行者本身
   不行」也要降级 —— 差别只可能在**交给它的那份 state / COW**。
3. **包里的 `boot`/`vbmeta` 内容本身能启动**：系统 OTA 写进去的 `boot_a`/`vbmeta_a`（同一份包的
   内容）起得来。于是「A 槽镜像 / AVB 起不来」排到最后。

**证据链（2026-11-23，dropbox 里 8 条 `SYSTEM_BOOT`）**

| 时间 | 系统 | 读法 |
| --- | --- | --- |
| 21:21:42 | OS4.0.0.11（内核 `g-c4127a25dcf3`，7-15 版） | 装包之前的系统 |
| 21:38:28 | OS4.0.4.0（内核 `g-0908354dac1b`，8-20 版） | 4.0.4.0 第一次起来 |
| 21:41:53 / 21:44:51 / 21:49:22 / 21:56:15 | OS4.0.4.0 | **同一晚共 5 次，间隔 3–7 分钟** |
| 22:28:13 | OS4.0.0.11 | 回到旧系统（VAB 的失败回退） |
| 22:52:23 | OS4.0.4.0 | 系统 OTA 装完，最终成功 |

每条内容都是 `isPrevious: true` + Build + Hardware: sun + Radio + Kernel，这版 dropbox 不记
`boot_reason`。关键在次数：**这 5 条每条都写成功，说明每次启动都到了 `system_server`。**所以
「卡 logo」的说法要改：那次 rec 装进 A 槽的系统**不是起不来，是起来了又倒、倒了被拉起、5 次之后
被引导器回滚到 B**（22:28 那条 4.0.0.11 就是回滚）。boot / 内核 / vbmeta / system 全过了 ——
§7.2 的「镜像起不来」再被排除一次。

**已排除（2026-11-23 逐条核到代码）**

- **`reflash()` 不碰 `boot`/`vbmeta`**：`keep.rs:15` `const PARTITION = "recovery"`，`keep.rs:66-67`
  只把 `image::device(dir, PARTITION, slot)` 交给 `image::flash` —— 两槽 recovery，别的分区没有。
- **`installed_boot_set()` 在这台机上是 no-op**：调用点 `run.rs:608`（apply 之前），实现在
  `run.rs:554` → `ensure_installed()`（`skyward-systems/src/install.rs:378`），只有 `state.occupied`
  有值才动作；这台机没装过第二系统，所以每次都是空转。
- **merge 执行者没问题**：首启的 `init` + `snapuserd`；系统 OTA 已经用它把 `Snapshotted` 合到
  `None`，而 recovery 内装完根本不跑 merge。
- **包里的 `boot`/`vbmeta` 内容能启动**：系统 OTA 写进去的是同一份包的内容。

**排序**

| 排序 | 机制 | 为什么排这里 | 判据 |
| --- | --- | --- | --- |
| 1 | **我们交给首启的那份 COW / state，与首启的系统视图对不上** —— 合并期出错或合不完，于是每次起来之后被拖倒 | 唯一能解释「同包、同机、同槽，OTA 成、我们不成」的差异：OTA 的 COW 是 vold 解密后由引擎自己写在 `/data` 上；我们的 COW 是我们**自己的 ext4 挂载**写下去的 | 每次起来后 3–7 分钟内倒、5 次后被回滚；**第一次倒就进 rec** 看 state 是否仍 `Snapshotted`/`Merging`、`snapshots/` 还在；同时看 `misc` 里 A 槽的 `retry_count` 归零没——归零就说明回滚已经跑完，那时 state 一定被清，不能拿它判「merge 没推进」 |
| 2 | **`sync` 的边界与首启视图是否一致**（见下，第 1 条的一个具体成因） | `power()` 只写 `sys.powerctl`，落盘由 init 负责（`lib.rs:146` 注释：`init syncs and unmounts before it acts`）。能问的只是：那次 sync 覆不覆盖我们那份 COW 所在的挂载；以及 update_engine 自己 unmount `/metadata` 时是否已经把它的视图带走 | 与第 1 条同（都只表现成 merge 一动不动）；分开得看装完以后 `/proc/mounts` 里 `/data` 是谁挂的、dm 状态如何 |
| 3 | **A 槽镜像 / AVB**（排末，基本可排除） | 被反证两次：同 `boot_a`、同 `vbmeta_a` 的 OTA 起得来；且 dropbox 证明那 5 次都到了 `system_server`。只剩一种残留：我们那次写下的 `vbmeta` 与 OTA 写的不一致 | state 已 `None`、`snapshots/` 清空，而系统仍起不来 |

**一条能量化的旁证**：`metrics-merge-start-duration = 78` 是 OTA 那次的读数 —— **单位未核**，只当
引擎自己记的量级看，而且它记的是 merge 相关的一段耗时，**不等于**完整 merge 全流程。它的用处是
相对比较：这个量级远小于那 5 次「3–7 分钟才倒」。所以下次「第一次倒就抓 state」有了参照：

- state 仍 `Snapshotted`、`snapshots/` 完整 ⇒ merge 长时间没有推进（远超 OTA 那次的量级）；
- state 已进 `Merging` 或带失败码 ⇒ merge 真在跑，但出错。

两条都别当绝对时间用。

**关于第 2 条的措辞**：原来说「重启前没落盘」不准确 —— 落盘不是没人管，是 init 在 `sys.powerctl`
之前 `sync` 并 unmount。所以收窄成「sync 的边界与首启视图是否一致」，而它的两个答案（sync 没覆盖
那个挂载 / 引擎卸 `/metadata` 时带走了视图）都落回第 1 条：**首启拿到的输入与我们写下的不一致**。
两条的判据因此不分家，别指望能分开。

内核侧证据（pstore）在这台机上已确认拿不到（§7.4 第 2 条），所以排序只能靠「state 有没有被动过」
加框架侧痕迹来判。

### 7.3 判别实验的定位（已改）

它不再用来「证明 merge 链通不通」——那条已经由系统 OTA 证明。现在它只回答一件事：
**我们这条路径写出来的 A 槽，不带快照，能不能起。**

| B 组（`adb sideload`，无快照）的结果 | 读法 |
| --- | --- |
| 进得了系统 | 病因锁死在**快照线**（§7.2 第 1/2 条）。干净覆盖写能起 ⇒ 我们写进去的镜像与 AVB 都对，差别只在 COW 与 state |
| 照样起不来（起来又倒） | 连不带快照的干净 A 槽都起不来 ⇒ 问题不在快照，而在**我们写进去的东西**或**写完之后做的动作**。核两样：① `vbmeta_a` 与包内那段是否逐字节一致；② overwrite 分支下 `boot`/`vbmeta` 是 update_engine 直接写的，不是我们写的（`reflash` 只写 recovery，`installed_boot_set()` 在这台机是 no-op，两条已在 §7.2 排除） |

步骤、前置、判据、记录表仍在 **`docs/exp-404-sideload.md`**，照抄即可跑。主判据一行：装完后
`recovery.log` 的 `left for the next start` 从 `Snapshotted, merge outstanding true` 变成
`snapshots None, merge outstanding false`。

B 的代价：直接覆盖写 A 槽，失败时 A 槽起不来（已有先例，重刷可回）；`init_boot_a` 被包内原厂
镜像覆盖，**A 槽的 KernelSU 会没**，实验后要重刷补丁。

### 7.4 现场已核（2026-11-23，手机插回后实测）

1. **`/metadata/ota` 干净**：`state` 0 字节、`snapshots/` 空、`update_engine/` 子目录不存在。
   **这是 22:28 回滚与 22:52 OTA 成功之后的终态，不代表装完当场的样子**（装完当场是 state 74 字节
   + 9 个快照目录）。⇒ 开头担心的 `Refusal::Pending` 不存在，下次装包不会被「更新已写入，等待合并」
   当场拒；也印证「失败的那次启动让 VAB 状态走了标准作废流程」——**这也是为什么现场必须在下一次
   重启（以及回滚）之前拷走**（§7.5）。
2. **`/sys/fs/pstore/` 是空目录**（`total 0`，只有 `.` 和 `..`，而且连这两个条目的时间戳都是 1970
   年）：等于**后端根本没注册**，不是"这次没留"。那次的内核日志已经拿不到，**pstore 不再作为取证
   路线**，取证只用下面这些实测过的源。
3. **A 槽现在的 4.0.4.0 是好的**：`sys.boot_completed=1`、`ro.boot.slot_suffix=_a`、
   `bootreason=reboot,userrequested`、`veritymode=enforcing`（原样记录）。这套内容来自系统 OTA。
4. **11-22 那次安装确实写到了 `init_boot_a`**：装完当场读到原厂 `6e636c81…`，现在是 `7c1453fa…`
   的 KSU 补丁。⇒ 写入路径（`image::flash` + `make_writable`）在 4.0.4.0 上是对的，病因不在
   「写错内容」。

分区读回（11-23，sha256 前 16 位）：

| | 槽 A | 槽 B |
| --- | --- | --- |
| boot | `d2d363b2619a9083` | `55af7f87a00d3496` |
| init_boot | `7c1453fa4641fab6`（KSU） | `1cfa51eaa7de6888` |
| recovery | `3676c676ebdd82c6`（v1.0.13） | `3676c676ebdd82c6`（v1.0.13） |
| vbmeta | `dd457f4ca98d9081` | `e6acbd65679085bf` |
| dtbo | `f690924dda49e37c` | `6e60d8d4b952d732` |
| vendor_boot | `42fd42055df23e36` | `0d3b89fbd9fbbf1a` |

**证据源实测（2026-11-23）**

1. **`/data/system/dropbox/` —— 唯一硬证据**：8 条 `SYSTEM_BOOT`，时间线见 §7.2。条目内容都是
   `isPrevious: true` + Build + Hardware: sun + Radio + Kernel，这版**不记 `boot_reason`**。
   `SYSTEM_LAST_KMSG` 的上游是 pstore（空），别指望有内容。这些文件带 DropBox 自己的头 + gzip，
   别直接 `zcat`，拷出来解析。
2. **`/data/misc/bootstat/` —— 实测废**：30 来个文件全是 0 字节，mtime 大多是 1970/198x 的纪元
   垃圾，略有用只有两个 mtime（`boot_complete` 1970-01-01 22:37、`build_date` 2026-11-13 00:46）。
   只当空标记看，不作证据源。
3. **`logcat -b all -d` —— 只覆盖当前这次启动**：228,822 行里 `snapuserd` / `update_engine` /
   `vabc` / `virtual a/b` / `COW` / `skyward` / `metadata-ota` **全部 0 命中**，环形缓冲只到
   11-23 11:21 之后，11-22 晚上已经滚没了。不作证据源。
4. **`/data/misc/update_engine/prefs/` —— 比 dropbox 更值钱的账本**。系统 OTA 那次留下的原文：

```
previous-version = Xiaomi/bixi/bixi:17/CP2A.260605.016/OS4.0.4.0.XOHCNXM:user/release-keys
update-state-next-operation = -1          Idle，没有挂起的操作
metrics-merge-start-duration = 78
metrics-boot-complete-timestamp = 53388
delta-update-failures = 0
current-bytes-downloaded = 0 / total-bytes-downloaded = 0
manifest-bytes = 339672
```

   时间戳：`manifest` 与 `update-check-response-hash` 22:16 → `delta-update-failures` 22:21 →
   merge 相关三件 22:27 → `boot-id` 22:51 → `boot-complete` 22:52。**`merge-start-duration=78`
   加 `next-operation=-1` 就是 merge 走完的实证**，与 §7.2 的定案对得上。rec 内装包时引擎会把同一套
   记账写进同一个目录（`/data` 开着时），所以下次复现要连着这个目录一起拷（§7.5）。

### 7.5 下次复现的现场保全清单

**红线：装完 → 拷现场 → 确认拷全 → 才允许重启。** 失败的那次启动一结束，VAB 的作废流程就把
`/metadata/ota` 清成现在的样子（已实测）；重启同时会带走 `/skyward/run` 里的两份日志（tmpfs）。
顺序反了，这一轮就等于白跑。

**一次性脚本**（在 rec 的终端里跑，或 `adb shell` 进去跑）：

```sh
OUT=/data/local/tmp/skyward-ota-$(date +%m%d%H%M)
mkdir -p $OUT
# 1 快照状态：原样 + 十六进制（0 字节与 None 是两件事，都要留）
cp /metadata/ota/state $OUT/state 2>/dev/null
od -An -tx1 /metadata/ota/state > $OUT/state.hex 2>/dev/null
# 2 快照目录：清单 + 整份
ls -laR /metadata/ota/snapshots > $OUT/snapshots.ls 2>/dev/null
cp -a /metadata/ota/snapshots $OUT/ 2>/dev/null
# 3 update_engine 子目录（有才有）
ls -laR /metadata/ota/update_engine > $OUT/update_engine.ls 2>/dev/null
cp -a /metadata/ota/update_engine $OUT/ 2>/dev/null
# 4 两份 tmpfs 日志
cp /skyward/run/update_engine.log $OUT/ 2>/dev/null
cp /skyward/run/recovery.log $OUT/ 2>/dev/null
cp /skyward/run/install-script.log $OUT/ 2>/dev/null
# 5 内核侧（dmesg 这会儿还在，重启就没）
dmesg > $OUT/dmesg.txt
# 6 挂载与 dm 状态
cp /proc/mounts $OUT/mounts.txt
ls -l /dev/block/mapper > $OUT/mapper.ls 2>/dev/null
dmsetup ls --tree > $OUT/dm.txt 2>/dev/null
# 7 引导器的槽元数据（重试计数 / 成功标记 / 回滚都在这里）
dd if=/dev/block/by-name/misc of=$OUT/misc.bin 2>/dev/null
od -An -tx1 $OUT/misc.bin > $OUT/misc.hex 2>/dev/null
# 8 引擎在 /data 上的记账（/data 开着时才有）
cp -a /data/misc/update_engine      $OUT/update_engine-data 2>/dev/null
cp -a /data/misc/update_engine_log  $OUT/update_engine-logs 2>/dev/null
# 9 刚写进去的分区哈希
for p in boot_a init_boot_a vbmeta_a dtbo_a vendor_boot_a recovery_a ; do
  printf '%-14s ' $p ; sha256sum /dev/block/by-name/$p | cut -c1-16
done > $OUT/partitions.txt
ls -lR $OUT > $OUT/manifest.txt
```

`misc.bin` 的读法：里面是 bootloader 的 `boot_control` —— A/B 两槽各自的 `retry_count`、
`successful` 标记与 `merging` 位，VAB 要不要回滚全靠它判。**A 槽 `retry_count > 0` 或
`successful=false`，说明那几次重试记录还在**，可以直接和 dropbox 的 5 条 `SYSTEM_BOOT` 对上；
**`retry_count` 已归零，则说明回滚流程走完了**，那时 `/metadata/ota` 的 state 一定被清过，别拿它
判「merge 没推进」。

取走：

```powershell
adb pull /data/local/tmp/skyward-ota-XXXX ./
```

- 这台 rec 的 adb 若没实现 `pull`（报 `unknown service`），改走 MTP：`/data` 开着时落点换
  `/sdcard`，然后在 Windows 侧用 Shell COM（`Shell.Application` 的 `NameSpace`）直接从手机拷出来
   —— 不在手机上折腾文件。**不要因为 pull 不顺就把手机重启掉。**
- 「拷全」的判据：`manifest.txt` 里每一项都有大小；`state.hex` 不是空文件；`update_engine.log`
  不是 0 字节；`misc.bin` 非 0 且 `misc.hex` 有内容；`partitions.txt` 六行齐；`/data` 开着的话
  `update_engine-data/` 里有文件。

**第一次倒之后就拷，不必等回滚。** dropbox 已经证明那次安装的系统每次都能跑到 `system_server`
（§7.2 证据链），所以「起来 → 倒了」这个瞬间之后进 rec，`/metadata/ota` 里的 state 很可能还是活的；
等 5 次重试耗尽、被回滚到 B 槽，那条作废流程就把现场清掉了。

拷完之后才允许重启；重启后才看首启。若起来又倒，再进 rec 按 §7.4 的证据源取证。


