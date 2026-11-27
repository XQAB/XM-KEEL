# recovery 里直接用 QMI 读传感器：需要什么，缺什么

> 机型 bixi（MIX Flip 2，SM8750）。目标：**不进 Android、不依赖 sensors HAL 与 framework
> service**，用内核的 QRTR socket 直连 ADSP 上的传感器服务（SSC，service 400），拿到样本。
> `sscrpcd`／`pd-mapper` 这条 daemon 链由 recovery 自己拉起（见 §5.1），不需要 init 的
> `vendor.sensors` service。本文是「让 SSC 在 recovery 里可用」的条件清单，配套实现在
> [`crates/keel-sensors`](../crates/keel-sensors) 与 [`crates/keel/src/sensors.rs`](../crates/keel/src/sensors.rs)。

## 0. 结论（三句）

1. **不需要 framework。** QRTR socket 是内核特性，name service 由内核的 dispatcher 应答，
   回复直接给出 SSC 的地址（本机实测 `node 5 / port 0x12`）。本仓库的 `keel-sensors`
   已经用这套链路在真机上读到 `hinge_angle`／`flip_status`／`ambient_light` 的真实样本。
2. **registry 的大小跟三个 daemon 是否就位有关，跟 SSR 无关。** 实测（三次独立冷启动）：
   只挂分区、insmod、写 ld.config 而不起 daemon，registry `31 uids`；
   `pd-mapper`／`sscrpcd` 起齐后 **2 秒 140 条、5 秒 142 条、60 秒仍是 142**；
   这时写不写 `/sys/kernel/boot_adsp/ssr` 都是 142。ADSP 重启会清空 registry 这一条依然成立，
   但 recovery 的冷启动路径并不需要靠写 `ssr` 去"换回"它。
3. **所以 recovery 里第一件要查的事，是 daemon 有没有挂上 ADSP**（`sscrpcd` 的
   `apps_dev_init` 是否 EBUSY），而不是 shutdown 开关。`init.recovery.qcom.rc` 的
   `on fs` 段（小米 `Recovery_StabilityEnhance`）那一发 `write /sys/kernel/boot_adsp/ssr`
   是音频子系统的重启开关，写它**不带来 registry，也不带走 registry**；何况
   `vendor.sensors` 这个 service 在 rc 里是 `disabled`，冷启动本来就没有人起它。
4. **这套 daemon 链不是外屏切换的前置条件。** 换屏判据先读触摸驱动
   （`/sys/devices/platform/synaptics_tcm*/capfold_status`，设备树里的文件，不经 ADSP），
   拿不到才问 ADSP 上的 hinge。零改动冷启动实测（什么都不敲，只读轮询）：

   ```
   up=26s  fold=[capfold_status: 0]  goodix=1  p0=0  p1=819
   up=32s  同上
   up=92s  同上
   ```

   这时 `pd-mapper`、`adsprpcd`、`sscrpcd` 一个都没起，`/proc/misc` 里没有 fastrpc 项，
   `/vendor/dsp` 没挂、`/persist` 没 bind——`fold=0`、内屏灭（`p0=0`）、外屏亮
   （`p1=819`）、外屏触摸驱动已加载（`goodix=1`）全部成立。所以 §5.1 那六步解决的是
   「触摸驱动不给答案时没有第二个判断来源」，不是「外屏才能切」。

## 0.1 这次改动在换屏路径里的位置

`crates/keel/src/ui.rs` 里 `shut_now(&flip)` 是 `fold_status().or_else(|| flip.folded())`：

- 第一来源是触摸驱动，`fold_status()` 直接读 sysfs，**今天就能用，不依赖本文任何一步**；
- 第二来源才是 `hinge_angle`，只有驱动文件读不到（或内容不认识）时才被问到；
- `sensors::bring_up()` 因此是 best-effort 的**兜底**，失败只写 kmsg、不影响启动，
  更不能被写成「外屏切换的前置条件」——不然以后读代码的人会以为换屏依赖 ADSP。

## 1. 判定顺序（按这个顺序查，别跳）

1. `cat /sys/class/remoteproc/remoteproc1/state` —— `running` 才继续。
2. `ps -A | grep -E 'pd-mapper|sscrpcd'` —— **两个都在**才有 registry。只有 `pd-mapper`、
   或 `sscrpcd` 日志里每 30 秒 `Error 0x200: apps_dev_init failed ... EBUSY`，都等于没挂上。
3. 用 `keel-sensors` 的 `Client::list()` 查 `registry`：**140 上下 → registry 在；
   31 条左右 → daemon 没挂上或环境没铺好；只有 `registry` 一条 → ADSP 被重启过。**
4. 列表正常后，再按名字取 sensor uid，再使能、读样本。

`/sys/kernel/boot_adsp/ssr` 不必看、也不必写：它不是 registry 的来源，也不是缺失的原因。

## 2. 必须挂上的分区

| 挂载点 | 来源 | 为什么 |
| --- | --- | --- |
| `/vendor` | dm（`/dev/block/mapper/vendor`） | SSC 的固件入口、`/vendor/etc/sensors/*`、`/vendor/dsp` 下的 skel |
| `/odm` | dm | 传感器 JSON（`/odm/etc/sensors/config/json.lst`）在这里 |
| `/vendor/dsp` | `sde11` ext4 | ADSP 侧的可执行与 skel，`pd-mapper` 要从这里找 |
| `/mnt/vendor/persist` | `sdf7` ext4 | sensor 的 persist 目录（缓存用，非注册表来源） |
| `/vendor/firmware` | `sde7`（bind 到 `/vendor/firmware_mnt/image`） | `adsp.mdt` 等固件 |

## 3. linker 命名空间

recovery 默认系统分区挂在 **`/mnt/system`**（不是 `/system_mnt`，这个坑踩过）。
这份 545 字节的 `ld.config.txt` 已经固化进仓库：`crates/keel/src/sensors/ld.config.txt`，
启动时由 `sensors::bring_up()` 原样写到 `/linkerconfig/ld.config.txt`（内容相同就跳过），
**不在运行时拼字符串**。全文：

```
dir.recovery = /system/bin
dir.vendor = /vendor/bin
dir.sysbin = /mnt/system/system/bin
[recovery]
namespace.default.isolated = false
namespace.default.search.paths = /system/${LIB}
namespace.default.permitted.paths = /system
[vendor]
namespace.default.isolated = false
namespace.default.search.paths = /mnt/system/system/lib64/bootstrap:/mnt/system/system/lib64:/system/${LIB}:/vendor/${LIB}:/vendor/${LIB}/hw:/odm/${LIB}
namespace.default.permitted.paths = /vendor:/odm:/product:/system_ext:/mnt/system:/system:/data:/tmp:/persist:/mnt/vendor
```

少了 `bootstrap` 那一段，`pd-mapper`／`sscrpcd` 会在 `libc++`／`libutils` 上起不来；
少了 `/mnt/system/system/lib64`，`libc.so` 找不到。全文 545 字节、末尾一个换行，与手机上那份
逐字节相同（sha256 `48bf1027bf48119bce36cdc1637364a06eb9136a2458218bc8c1b04c016faddf`）。

## 4. fastrpc 设备

```sh
insmod /vendor/lib/modules/frpc-adsprpc.ko            # 模块名不是 fastrpc
grep fastrpc-adsp-secure /proc/misc                   # 本次启动是 282
[ -e /dev/fastrpc-adsp-secure ] || mknod /dev/fastrpc-adsp-secure c 10 "$(grep fastrpc-adsp-secure /proc/misc | cut -d' ' -f1)"
chmod 666 /dev/fastrpc-adsp-secure
```

**minor 每次启动都会变**（见过 281、282），节点也可能由内核自己建好——所以代码里既不写死
minor，也不无条件 `mknod`：先看 `/dev/fastrpc-adsp-secure` 在不在，不在才从 `/proc/misc`
读 minor 补建。`sscrpcd` 起 listener 需要这个节点；没有它，`adsp_default_listener_start` 直接失败。

## 5. 进程顺序

```
pd-mapper  →  adsprpcd rootpd adsp  →  sscrpcd sensorspd adsp
```

- `pd-mapper` 先把 ADSP 上的 protection domain 映射做完，`sscrpcd` 才能打开 listener；
- **名字是 `adsprpcd`，不是 `adsrpcd`**（少一个 p 就是另一个不存在的路径；`/vendor/bin` 里
  实际有 `adsprpcd` 与 `audioadsprpcd`）。这一环是可选的：本机 A/B 里它是**路径不存在、
  被静默跳过**跑完的，registry 照样 142——也就是说 bixi 上 `pd-mapper` + `sscrpcd` 就够；
  早期版本把名字写错成 `adsrpcd` 并靠 `needed: false` 静默跳过，日志干净得像一切正常，
  `sensors.rs` 现在对「程序不存在」无论 optional 与否都写一条 kmsg；
- `sscrpcd` 只是 fastrpc listener 的宿主，**它不提供注册表**：停掉它再读，样本照旧；
- 三个都以 `setsid` 起在自己会话里，recovery 退出或 adb 断线都不带走它们。

## 5.1 冷启动六步（实测，一步不多）

```sh
mount -t ext4 -o ro /dev/block/by-name/dsp_a /vendor/dsp
mkdir -p /persist && mount --bind /mnt/vendor/persist /persist
insmod /vendor/lib/modules/frpc-adsprpc.ko
# 节点：没有才建，minor 从 /proc/misc 读（见 §4）
cp ld.config.txt /linkerconfig/ld.config.txt
setsid /vendor/bin/pd-mapper              >/dev/null 2>&1 &   sleep 1
setsid /vendor/bin/adsprpcd rootpd adsp   >/dev/null 2>&1 &   sleep 2   # 没有这个二进制就跳过，跳过了也写 kmsg
setsid /vendor/bin/sscrpcd sensorspd adsp >/dev/null 2>&1 &
```

三次独立冷启动（uptime 15 s / 31 s / 41 s 起跑）的 A/B：

| 做法 | registry | hinge |
| --- | --- | --- |
| 什么都不做 | 空（`/proc/misc` 里都没有 fastrpc 项） | `found hinge_angle: 0` |
| 只挂 dsp/persist + insmod + ld.config，不起 daemon | 31 条 | `found hinge_angle: 0` |
| **六步全做（不写 ssr）** | **2 s 140 条，5 s 142 条，60 s 仍 142** | **`found hinge_angle: 1`，样本 `[0.0, -0.00339, -0.00095, 2.0, 0.0]`** |
| 六步 + 写 ssr | 142 条 | 正常（ssr 不是必要条件） |

**结论**：31 → 142 取决于三个 daemon 是否就位，与 `/sys/kernel/boot_adsp/ssr` 无关。
那节点是音频子系统的重启开关，**recovery 里一行都不要写**；等 5 秒就够。

这套顺序编成了 `crates/keel/src/sensors.rs` 的 `bring_up()`：起屏之后、`FlipController::start()`
之前调用一次，每一步 best-effort（失败只写 kmsg），服务已经在答就直接返回。它的定位是
**给换屏一个第二判据**（见 §0.1）：触摸驱动在的时候用驱动，驱动不在或答不上来时才有它，
它不决定外屏能不能亮。

## 6. 协议（客户端部分，`crates/keel-sensors/src/frame.rs`）

- socket：`AF_QIPCRTR = 42`，`SOCK_DGRAM`，不 bind，`getsockname` 得 `node 1 / port 0`；
- name service 端口 **`0xfffffffe`**；LOOKUP 20 字节 `cmd=0x0a, service=400, instance=0, node=0, port=0`，
  回复 `cmd=4` → `node 5, port 0x12`；
- 请求帧：`00 <txn> 00 20 <len:BE16>` + `00 01 <tlvlen:LE16> <pblen:LE16>` + protobuf + 可选尾 `10 01 00 01`；
  msgid 与 `len` 是大端，TLV 那两个长度是小端；
- **by-name 查询的名字在第二体的 `field 2` 里，不是 `field 1`**：HAL 的实帧是
  `22 <len> 12 <len> 0a <len> <name> 10 01 18 00`。放在 `field 1`（`0a <len> <name>`）一律回
  `result=1 / error=7`（POLICY）——这条岔路踩了很久，和尾 TLV 无关；
- **尾 TLV 只是 per-request 开关**，两种都能答：by-name 的 61 字节（无尾）与 65 字节（带尾）
  换来同一份 91 字节答案；配置类按 HAL 的做法带上；
- **空名字 = 整张 registry**：一次回复 3065 字节、150 条 uid，**名字是空的**（名字要靠 `msgid 1`
  读 `attr 2` 补），所以「列全部传感器」是一次往返；
- 回复帧 `04 <txn> 00 21`：`01 08 00 <ind>` + `02 <len> <sublen> <pb>`，
  pb 里 f1 = suid、f2 = `0d <msgid:LE32> 11 <seq> 1a <len> <payload>`；
  **frame 头里声明的长度与 pad 的位置并不统一**，所以实现里是按 `01 08 00` 找体，而不是按长度切；
- **回帧头里的 `txn` 是 ADSP 自己的计数器，不回显请求的 `txn`**：同一个发问连发几次，答案的 `txn`
  是 1、3、5、7 这样自己往上走。所以答案只能按「inner msgid + 体里的 suid」配对，按 `txn` 配对
  会把上一问的答案当成本问的答案（实测把 gravity 的状态码记到 ori 头上，accel 也「读到」了别人
  的样本）。配对失败就是 `watch` 里串传感器。
- 配置的答案就是第一个样本：能跑的传感器回 inner **1025**（payload 里 `0a <len>` 是 packed
  float32，不缩放，原值）；不能跑的回 inner **130** + 一个 code（这台机：accel 3、gravity 2、
  ori 6、gyro 3）。on-change 类传感器之后只在数值变化时才再发，静止时收不到第二个样本。

## 6.1 属性表（`msgid 1`）

请求体是 `08 01 10 00`，**第二体必须是空的存在（`22 00`）**：给 `None`（连 `22 00` 都不写）时
150 条 uid 一律回 `error 7`（POLICY）。答案 `msgid 128`，每条是
`0a <len> 08 <id> 12 <len> <value>`，`value` 本身是 `0a <len> <one of …>`：

- 字符串：`12 <len> <text>`（厂商的 16 字节名字后面带一个 `00` 填充，取到第一个不可打印字节为止）；
- 数字：`21 <8 字节 LE>`（field 4，wire 1）或 `28 <1 字节>`（field 5，varint）。

这台机上读出来的编号：

| id | 含义 | 例（`hinge_angle`） |
| --- | --- | --- |
| 0 | 厂商私有名 | `oem_hinge_angle` |
| 1 | 厂商 | `XiaoMi(V3.2)` |
| 2 | **服务用的名字** | `hinge_angle` |
| 5 | data type | `sns_std_sensor.proto` |
| 16 | stream type | 0 = 连续周期，1 = on-change |
| 3、4 | 间隔/上报参数 | 1 |

`gravity`（`a74398c0…/c5d45a2d…` 与 `…/c6d45a2d…`）这条比较特殊：`msgid 1` 只回 ACK、
不回 128 实例，所以连 stream type 都读不出来；它发 514 回 `inner 130 code 2`。这和 accel 不同，
accel 的属性表是完整的。

## 7. 这台机器上能读什么、不能读什么

| 传感器 | uid（低/高） | 结果 |
| --- | --- | --- |
| `ambient_light` | `088285bf47d64e7d` / `b71d3ab9d3c58e2d` | 3 float（lux），实测 25.2 |
| `hinge_angle` | `a3b53fe2753556e1` / `12624a343a91e372` | 5 float，`[0]` 角度，实测 179.0 |
| `flip_status` | `191b30ebf1194817` / `95682728dd533367` | 12 float，`[1]` = 179.0 |
| `device_orient` | `eef435f9287516a7` / `a1964b507cf7e3e0` | 16 float，恒定 `4,4,0…` |
| accel / gravity / ori / gyro | 见 issue #5 | 514 直接回 inner 130 + code：3 / 2 / 6 / 3 |

accel 的失败不是缺消息号，也不是属性表空：四个 accel uid 的属性表是完整的（Android 侧 150 条
全量 inventory 实测）：

| attr | 值（`3846c24e`，lsm6dsv） |
| --- | --- |
| 0 | `lsm6dsv` |
| 1 | `STMicro` |
| 2 | `accel` |
| 4 | 28676 |
| 5 | `sns_accel.proto` |
| 6 | rates `[30, 60, 120, 240, 480]` |
| 7 | `[0.000598, 0.001196, 0.002393, 0.004786]` |
| 8 | 240 |
| 9 | `(17, 100, 190, 3)` |
| 10 | 3 |
| 12 | OFF × 4 |
| 16 | 0（continuous periodic） |
| 22 | `(3, 2, 1)` |
| 25 | 960（量程 g） |

`3c4b340b`（lsm6dso）的 rates 是 `[26, 52, 104, 208, 416]`。真凶是 `sns_accel.proto` 这个厂商私有
类型的 **config 内层 msg id**：不在 `sns_std_sensor_config`（512），也不是 513 / 514。4 个 uid ×
6 种组合（512 的 f3=f32、512 的 f1=nested f32、512 的 enable+f3=f32、513 on-change、514 on-change
30、514 on-change 200）全试过：512 三条 `error 7` / 无答案，513 与 514 全是 `ack` + `inner 130`。
recovery 与 Android 两侧结果一致。三轴放弃，外屏翻转改用 `hinge_angle`。

## 7.1 recovery 里到底剩了什么（`507d820c` 实测，recovery 下 adb root）

recovery 下 registry 只有 **10 条**，逐个读 `attr 2`：

```
suid / aoc / tp_hall / client_sensor / device_mode /
camera_face_detect / camera_hand_detect / camera_qrcode_detect / camera_frame / wifi_svc
```

`hinge_angle`、`flip_status`、`ambient_light`、`tilt` 全不在。**`tp_hall` 在，而且能跑**：

- `attr 0/1/2 = TP Hall / xiaomi / tp_hall`，`attr 5 = sns_std_sensor.proto`，`attr 16 = 1`（on-change）；
- 514 打开直接回 `[1.0, 0.0, 0.0]`，是当前状态值，不报 130。

所以 recovery 侧的翻转判据可能根本不需要 `hinge_angle`：`tp_hall` 是这套 registry 里唯一与开合
有关的传感器，一次折动就能定性（`1.0` 到底是开还是合，得靠折一次才知道）。这条比"把 150 条
registry 在 recovery 里重建出来"便宜得多。

同时量到两件与老结论冲突的事：

- `remoteproc-adsp` 在 `1.42 s` 就已经被内核上电（`powering up`），init 的
  `wait /sys/kernel/boot_adsp/ssr` 在 `3.33 s` 才等到节点，紧接着又是一次
  `powering up`。但后来的 A/B 表明：**这一发写不写，registry 都一样**（§5.1）；
- 当时那块机器上 `sscrpcd` 起不来：`logcat` 里每 30 秒一次
  `Error 0x200: apps_dev_init failed for domain 0, errno Device or resource busy`，
  `/dev/fastrpc-adsp-secure` 由 `adsprpcd` 与 `sscrpcd` 同时持有，`attachguestos` 只有一个。
  **10 条就是"sscrpcd 没挂上"的样子**：把环境铺好、只起 daemon 能到 31 条，daemon 挂上 ADSP
  才到 140 上下，142 是这台机器的满值。

## 8. `ssr` 那一行怎么处理

`init.recovery.qcom.rc` 的 `on fs` 里有这两行（第 54、55 行，MIUI `Recovery_StabilityEnhance`）：

```sh
    wait /sys/kernel/boot_adsp/ssr
    write /sys/kernel/boot_adsp/ssr 1
```

`/sys/kernel/boot_adsp/ssr` 是**音频子系统挂了之后重启 ADSP** 的开关，不是 sensor registry 的闸门。
A/B 已经把这层因果关系定死：写它不带来 registry，也不带走 registry（§5.1）。

因此：

- recovery 自己的启动路径（`crates/keel/src/sensors.rs`）**不写这个节点**，一行都不写；
- 客户端的语序也改成"daemon 起齐就算完"，不再有"最后写 ssr"这一步；
- `init.recovery.qcom.rc` 里那两行**暂时不动**，它属于产品侧取舍（删掉对 audio、充电、thermal
  都没有依赖，但那是另一件事），等外屏翻转接入验收完再定。

## 9. 已知的失败路径（别重试）

- 清空 `/mnt/vendor/persist/sensors/registry/registry` 再 SSR：注册表不回来，也不会写回文件；
- 停 `sscrpcd`／停 HAL／停 `qsap_sensors` 再 SSR：样本、注册表都不受影响，说明都不是提供者；
- 用 `bootmonitor.bootmode` 让 ADSP 按 normal 起：cmdline 只读，`/sys/kernel/boot_adsp`
  只有 `boot` 与 `ssr` 两个写节点，没有 bootmode 入口；
- 内核 IIO 直读：recovery 里没有 `i2c-dev.ko`／`spidev.ko`，`i2c-7` 的 `0x6a/0x6c` 是充电泵
  `sc8541`，`spi0/1/2` 没有 spidev。
