# recovery 里直接用 QMI 读传感器：需要什么，缺什么

> 机型 bixi（MIX Flip 2，SM8750）。目标：**不进 Android、不依赖 sensors HAL 与
> `sscrpcd`／framework service**，用内核的 QRTR socket 直连 ADSP 上的传感器服务（SSC，
> service 400），拿到样本。本文是「让 SSC 在 recovery 里可用」的条件清单，配套实现在
> [`crates/keel-sensors`](../crates/keel-sensors)。

## 0. 结论（三句）

1. **不需要 framework。** QRTR socket 是内核特性，name service 由内核的 dispatcher 应答，
   回复直接给出 SSC 的地址（本机实测 `node 5 / port 0x12`）。本仓库的 `keel-sensors`
   已经用这套链路在真机上读到 `hinge_angle`／`flip_status`／`ambient_light` 的真实样本。
2. **registry 只跟着冷启动来。** ADSP 首次起来时带完整注册表；**此后任何一次 SSR 都会把它清空，
   AP 侧没有任何进程能补回来**（`persist` 里的 json 片段只是缓存／输出，`sscrpcd`、HAL、
   `qsap_sensors` 都只是消费者）。实测：停掉 HAL 与 `sscrpcd` 再 SSR，注册表仍空；
   清空 `/mnt/vendor/persist/sensors/registry/registry` 再冷启动，注册表仍在——两边都不是来源。
3. **所以 recovery 里第一件要查的事，是 ADSP 有没有被重启过。** `init.recovery.qcom.rc`
   的 `on fs` 段（小米 `Recovery_StabilityEnhance`）每次开机都写一次
   `/sys/kernel/boot_adsp/ssr`，**这一发就等于把 registry 打空**，这是 recovery 里
   `registry` 列表只剩它自己、accel 属性为空的唯一根因。

## 1. 判定顺序（按这个顺序查，别跳）

1. `cat /sys/class/remoteproc/remoteproc1/state` —— `running` 才继续。
2. `cat /sys/kernel/boot_adsp/ssr` 有没有被写过、以及本机 init 里有没有那两行
   （`wait /sys/kernel/boot_adsp/ssr` + `write /sys/kernel/boot_adsp/ssr 1`）。
3. 用 `keel-sensors` 的 `Client::list()` 查 `registry`：**有几十上百条 → registry 在；
   只有 `registry` 一条 → 已被 SSR 清空，往下走也没意义。**
4. 列表正常后，再按名字取 sensor uid，再使能、读样本。

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
`ld.config.*` 的 `[vendor]` 段 `search.paths` 必须是：

```
/mnt/system/system/lib64/bootstrap:/mnt/system/system/lib64:/vendor/${LIB}:/vendor/${LIB}/hw:/odm/${LIB}
```

少了 `bootstrap` 那一段，`pd-mapper`／`sscrpcd` 会在 `libc++`／`libutils` 上起不来；
少了 `/mnt/system/system/lib64`，`libc.so` 找不到。

## 4. fastrpc 设备

```sh
insmod /vendor/lib/modules/frpc-adsprpc.ko     # 模块名不是 fastrpc
grep fastrpc-adsp-secure /proc/misc            # 本机是 281
mknod /dev/fastrpc-adsp-secure c 10 281
```

`sscrpcd` 起 listener 需要这个节点；没有它，`adsp_default_listener_start` 直接失败。

## 5. 进程顺序（全部 `system` 身份）

```
pd-mapper  →  adsrpcd rootpd adsp  →  sscrpcd sensorspd adsp
```

- `pd-mapper` 先把 ADSP 上的 protection domain 映射做完，`sscrpcd` 才能打开 listener；
- **少了 `adsrpcd rootpd adsp`，`sscrpcd` 的 ioctl 会 `EINVAL`**（实测）；
- `sscrpcd` 只是 fastrpc listener 的宿主，**它不提供注册表**：停掉它再读，样本照旧。

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
  `powering up` —— **那一发 `write ssr 1` 是一次多余的重启**，不是上电；
- `sscrpcd` 现在起不来：`logcat` 里每 30 秒一次
  `Error 0x200: apps_dev_init failed for domain 0, errno Device or resource busy`。
  `/dev/fastrpc-adsp-secure` 由 `adsrpcd` 与 `sscrpcd` 同时持有，`attachguestos` 只有一个，
  先开的赢。registry 只有 10 条更像是**注册表根本没下载进 ADSP**（sscrpcd 没挂上），
  而不是"SSR 把 150 条清掉了"。

## 8. 还没定的一条

`init.recovery.qcom.rc` 的 `on fs` 里有这两行（第 54、55 行，MIUI `Recovery_StabilityEnhance`）：

```sh
    wait /sys/kernel/boot_adsp/ssr
    write /sys/kernel/boot_adsp/ssr 1
```

`remoteproc1/state` 是 `running`，dmesg 也显示内核在 `1.42 s` 已经把 ADSP 拉起来了，所以这一发
是**多余的重启**：只删 `write … ssr 1`，保留 `wait` 就够。

```sh
exec root root -- /system/bin/sh -c '[ "$(cat /sys/class/remoteproc/remoteproc1/state 2>/dev/null)" = "running" ] || echo 1 > /sys/kernel/boot_adsp/ssr'
```

副作用评估：recovery 不跑 audio HAL（那唯一的运行时写者 `audiohalservice_qti.rc`
的 `vendor.audio.ssr.trigger` 不生效），充电走 `charger_fstab` + 内核 `power_supply`，
温度走内核 thermal，**删这一发对它们没有依赖**；唯一的耦合就是 ADSP 自己的固件加载。

但**删了也不等于 registry 会变成 150**：现在 `sscrpcd` 是 `EBUSY` 挂不上 ADSP，
注册表压根没下载。两步要分开验：先让 `sscrpcd` 挂上，再看 `ssr` 那一发是否还挡着。

## 9. 已知的失败路径（别重试）

- 清空 `/mnt/vendor/persist/sensors/registry/registry` 再 SSR：注册表不回来，也不会写回文件；
- 停 `sscrpcd`／停 HAL／停 `qsap_sensors` 再 SSR：样本、注册表都不受影响，说明都不是提供者；
- 用 `bootmonitor.bootmode` 让 ADSP 按 normal 起：cmdline 只读，`/sys/kernel/boot_adsp`
  只有 `boot` 与 `ssr` 两个写节点，没有 bootmode 入口；
- 内核 IIO 直读：recovery 里没有 `i2c-dev.ko`／`spidev.ko`，`i2c-7` 的 `0x6a/0x6c` 是充电泵
  `sc8541`，`spi0/1/2` 没有 spidev。
