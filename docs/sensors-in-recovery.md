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
5. **不做按重力的自动翻转：recovery 里没有重力数据源**，这是六个传感器逐一试出来的，
   不是没试到位。数据与代码落点见 §11。

## 0.1 这次改动在换屏路径里的位置

`crates/keel/src/ui.rs` 里 `shut_now(&flip)` 是 `fold_status().or_else(|| flip.folded())`：

- 第一来源是触摸驱动，`fold_status()` 直接读 sysfs，**今天就能用，不依赖本文任何一步**；
- 第二来源才是 `hinge_angle`，只有驱动文件读不到（或内容不认识）时才被问到；
- `sensors::bring_up()` 因此是 best-effort 的**兜底**，失败只写 kmsg、不影响启动，
  更不能被写成「外屏切换的前置条件」——不然以后读代码的人会以为换屏依赖 ADSP；
- 它跑在**独立线程**里（`ui.rs` 里 `std::thread::spawn(sensors::bring_up)`），首帧先画先上屏，
  服务晚几秒到只让 hinge 晚几秒有答案，不会把首页拖黑。

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

`dsp_a` 是这台机器的 sensor 固件分区；`dsp_b` 在 bixi 上是**另一个槽整块独立的 dsp 分区**，
不跟 slot 后缀走，挂上去会拿到旧槽的 skel 库，所以只是兜底顺序的第二位——代码里取到哪个会
以完整路径写进 kmsg（`sensors: {device} mounted as {fs}`）。

三次独立冷启动（uptime 15 s / 31 s / 41 s 起跑）的 A/B：

| 做法 | registry | hinge |
| --- | --- | --- |
| 什么都不做 | 空（`/proc/misc` 里都没有 fastrpc 项） | `found hinge_angle: 0` |
| 只挂 dsp/persist + insmod + ld.config，不起 daemon | 31 条 | `found hinge_angle: 0` |
| **六步全做（不写 ssr）** | **2 s 140 条，5 s 142 条，60 s 仍 142** | **`found hinge_angle: 1`，样本 `[0.0, -0.00339, -0.00095, 2.0, 0.0]`** |
| 六步 + 写 ssr | 142 条 | 正常（ssr 不是必要条件） |

**结论**：31 → 142 取决于三个 daemon 是否就位，与 `/sys/kernel/boot_adsp/ssr` 无关。
那节点是音频子系统的重启开关，**recovery 里一行都不要写**；等 5 秒就够。

这套顺序编成了 `crates/keel/src/sensors.rs` 的 `bring_up()`，在 `ui.rs` 里用
`std::thread::spawn` 起在**工作线程**上、紧跟 `FlipController::start()`：**首帧不等它**，
每一步 best-effort（失败只写 kmsg），服务已经在答就直接返回。它不阻塞首帧的原因很实在：
三个 daemon 的 settle 合计 3 秒，`wait_until_answering()` 再等最多 10 秒，摆在首帧前面就是
最坏 13 秒黑屏，而这个 recovery 首页原来是三秒左右亮的；放到线程里，菜单照旧先亮，hinge 的
1→8 秒退避重连会自己等来服务。

它的定位是**给换屏一个第二判据**（见 §0.1）：触摸驱动在的时候用驱动，驱动不在或答不上来时
才有它，它不决定外屏能不能亮。

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

**但这只是"六步没做"时的备选，不是常规路径。** 2026-11-27 的真机冷启动（§10）里六步由
`bring_up()` 自己走完、registry 到 **142 条**，`hinge_angle` / `flip_status` / `ambient_light`
全在灯下，`tp_hall` 也就不必再当主判据了——它只在 daemon 没挂上、registry 退回 10~31 条那种
残局里有意义。

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
## 10. 真机冷启动验收（2026-11-27，镜像 `skyward-5bcd20d`，sha256 `3dce0cf9…`，A 槽）

这一台机器、这一次冷启动，从开机到服务应答，**全程没有敲过一条命令**：六步由
`bring_up()` 在 `ui.rs:1656` 的线程里自己走完。

### 10.1 首帧不被挡

```
[10.278] set 51 backlight 200          ← 首帧画出
[10.300] set 51 backlight 0            ← 允许熄屏等按键（设计行为）
[10.667] sensors: bringing the sensor service up
```

服务拉起比首帧晚 **0.39 秒**。如果还是 `1c561fd` 那样同步调用，首帧必须等六步跑完
（这一次是 `15.745`），首页就是十几秒黑屏。`eddf395` 的 `std::thread::spawn` 确实把
这件事挪出了启动路径。

### 10.2 六步全程无人干预

```
[ 3.460] skyward: persist is mounted on /mnt/vendor/persist
[10.672] skyward: sensors: bringing the sensor service up
[10.674] skyward: sensors: /dev/block/by-name/dsp_a mounted as ext4
[10.684] skyward: sensors: frpc-adsprpc is loaded
[10.684] skyward: sensors: /dev/fastrpc-adsp-secure made, minor 281
[10.684] skyward: sensors: the linker namespace is written
[10.690] skyward: sensors: pd-mapper started as 757
[11.701] skyward: sensors: adsprpcd started as 823
[13.724] skyward: sensors: sscrpcd started as 836
[15.745] skyward: sensors: the sensor service answers
```

- **`persist` 那一行是 `main.rs` 的 `[partitions] mounted` 循环打的**（`bixi.toml` 里
  `mounted = ["persist"]`），不是 `bind_persist()` 挂的；`bind_persist()` 只是把它 bind 到
  `/persist`，所以源永远在。曾经怀疑过"recovery 里没人挂 persist"，这条日志把它否掉。
- **`minor 281` 是现读的**：`secure_node()` 从 `/proc/misc` 取 `fastrpc-adsp-secure` 的
  编号再 `mknod`，281 与 282 都见过，硬编码会错。
- 每一次 settle（`pd-mapper` 1 s、`adsprpcd` 2 s、`sscrpcd` 0 s）都落在预期位置。

### 10.3 一次折动的完整角度序列

`crates/keel-sensors/examples/hingewatch.rs` 抓到的 **209 个样本**（`0.02s → 62.88s`，
角度 min 0 / max 179）：

| 时刻 | 角度 | 说明 |
| --- | --- | --- |
| `35.58s` | `178°` | 开始合 |
| `36.08s` | `154°` | |
| `36.22s` | `124°` | |
| `36.31s` | `106°` | |
| `36.42s` | `89°` | 半折 |
| `36.50s` | `74°` | |
| `36.57s` | `61°` | |
| `36.73s` | `37°` | |
| `36.93s` | `16°` | |
| `37.21s` | `0°` | 完全合上（7.35 秒内 83 个样本） |
| `37.21→39.57s` | — | 静止 2.4 秒，无样本（on-change 语义） |
| `39.57s` | `2°` | 开始展开 |
| `41.68s` | `5°` | |
| `49.31s` | `167°` | |
| `62.88s` | `178°` | 完全展开（6.66 秒内 118 个样本） |

滞回阈值 `FOLDED_BELOW = 20.0` / `OPEN_ABOVE = 150.0` 在这批数据上判得干净：
0–20 段判「合」、150–179 段判「开」，中间段保持上一次判定。

### 10.4 外屏切换确实在工作，而且 `capfold_status` 会跟着折动变

同步采 `capfold_status` 与两块面板的背光（每 200 ms，变化才记）：

```
[33s] capfold=[capfold_status: 1] p0=819 p1=0      ← 按电源键亮内屏
[38s] capfold=[capfold_status: 0] p0=819 p1=0      ← 开始合盖，驱动先报 0
[38s] capfold=[capfold_status: 0] p0=0   p1=0      ← 内屏灭
[38s] capfold=[capfold_status: 0] p0=0   p1=819    ← 外屏亮
[43s] capfold=[capfold_status: 1] p0=0   p1=819    ← 开始展开，驱动回 1
[43s] capfold=[capfold_status: 1] p0=0   p1=0      ← 外屏灭
[43s] capfold=[capfold_status: 1] p0=819 p1=0      ← 内屏亮
```

**这一条更正了本文早先的一个判断**：`capfold_status` 在 recovery 里**是**会随折动变化的，
`shut_now()` 的第一来源（`fold_status()`）在合盖时真的拿到 `Some(true)`。先前看到「折了之后
仍是 1」，是因为那一次并没有真折（只是按键亮灭屏），不是驱动不更新。

`capfold_status` 真正的局限在别处：**它只有在触摸模块加载之后才存在**（`synaptics_tcm2` /
`xiaomi_touch`），冷启动最初几秒没有这个文件 → `fold_status()` 返回 `None`。`hinge_angle`
当兜底的意义就在这里，而不是「驱动永远不更新」。

折动时 `recovery.log` 还多了四行，正好对应 `ui.rs` 的 `load_cover_touch()`：

```
[4586.749761] loaded /vendor_dlkm/lib/modules/panel_event_notifier.ko
[4586.749959] loaded /vendor_dlkm/lib/modules/xiaomi_touch.ko
[4586.750148] loaded /vendor_dlkm/lib/modules/synaptics_tcm2.ko
[4586.750337] loaded /vendor_dlkm/lib/modules/goodix_core.ko
```

### 10.5 加速度计的精确拒绝码（比手测更细）

同一次冷启动里用 `enable(suid, 10)`（514）对每个候选逐个大写，回到的是：

| 传感器 | 结果 |
| --- | --- |
| `accel`（两个 uid） | `status 130, code 3` |
| `gravity`（两个 uid） | `status 130, code 2` |
| `gyro`（两个 uid） | `status 130, code 3` |
| `ori` | `status 130, code 6` |
| `tilt`（三个 uid） | `514 was acknowledged but never answered` |

**三轴不可得这条结论坐实**，并且现在有精确的 code 可查：不是"没消息号"这么笼统，
而是 ADSP 明确按传感器类别拒绝。`tilt` 更特殊：`ack` 之后永不回应，连错误码都不给。

### 10.6 诊断探针对 on-change 传感器的读取（已修，`84dc247`）

`keelbin sns` 当初对每个名字都发 **513（periodic）**，而 `hinge_angle` 的属性表写着
`attr 16 = 1`（on-change），只收 514，所以那条命令读不到它。`84dc247` 已改成按 `attr 16`
决定发 513 还是 514，真机验到 `attr 16: 1` 与 `sample: [179.0, …]`。本文这套链路一直不受
影响：`hinge.rs` 用的是 `client.read_sensor()`。

## 11. 重力翻转在 recovery 里不可行（实测结论）

**结论一句话：recovery 里读不到重力，所以不做按重力的自动 180° 翻转。** 页面朝向只有两个
来源——触摸驱动的 `capfold_status` 与 ADSP 上的 `hinge_angle`（§0.1），180° 翻转维持在设置页
的手动开关上。

六个候选逐个用 `enable(suid, 10)`（514，on-change）打，回的是：

| 传感器 | 结果 | 还有没有别的路 |
| --- | --- | --- |
| `accel`（lsm6dsv `3846c24e`、lsm6dso `3c4b340b`） | `ack` 之后 `status 130, code 5`；512 三条 `error 7`、无答案（§10.5 那次读到 code 3，两次都是拒绝） | 属性表完整（`sns_accel.proto`、rates 30/60/120/240/480、continuous、量程 960 g），但 config 内层 msg id 不在 `snsapi` 的标准枚举里，6 种组合 × 4 个 uid 零真样本 |
| `gravity` | `status 130, code 2`（两个 uid） | 同上，只是被拒得更早 |
| `gyro` | `status 130, code 3`（两个 uid） | — |
| `orientation`（`ori`） | `status 130, code 6` | — |
| `tilt` | `514 acked but never answered`（三个 uid），**0 样本** | 连错误码都不给，等于不存在的传感器 |
| `device_orient` | 能读，但只吐一条固定的 `[4, 4, 0 …]`：10 分钟里主动旋转，样本数 **1**，后续 payload 全 0 | 分不出 0°/90°/180°/270° |

另外两条也否掉了：

- **`screen_down`**：只认「屏幕朝下放桌上」这一种姿态，物理旋转（横过来、竖起来）完全不响应，
  拿它当翻转判据会把正常的握持也判进去；
- **内核直读**：recovery 里没有 `i2c-dev.ko` 也没有 `spidev.ko`，`/sys/class/sensors` 不存在，
  i2c-7 上的 `0x6a/0x6c` 是充电泵 `sc8541`，spi0/1/2 没有 spidev。绕 ADSP 这条路不存在。

所以代码侧把这件事定死：`crates/keel/src/orientation.rs` 里原来那套线程化的方向监听
（`start()` / `Sensor` / `Direction`）已经删掉——它们自 hinge 接手换屏后就没有调用者了，留着
只会让人以为还有重力自动翻转。现在这个文件只剩 `probe()`，只服务 `keelbin sns`，模块注释里
写着上面这张表的结论。

冷启动（首帧时间、六步日志、一次折动的 209 个样本、registry 142）的完整读数见
[`hinge-cold-boot.md`](hinge-cold-boot.md)。

