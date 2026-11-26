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
- **尾 TLV 是 per-request 开关**：配置类要带，`registry` 的 by-name 查询**不能带**
  （带了回 `error 2 / INVALID_TYPE`，实测 65 字节 vs 61 字节）；
- 事件帧：`04 <txn> 00 22 <len:BE16>` + `01 08 00 <ind>` + `02 <len> <pb>`；
  pb 里 f1 = suid 18 字节，f3.f1 = packed float32 数组（不缩放，原值）。

## 7. 这台机器上能读什么、不能读什么

| 传感器 | uid（低/高） | 结果 |
| --- | --- | --- |
| `ambient_light` | `088285bf47d64e7d` / `b71d3ab9d3c58e2d` | 3 float（lux） |
| `hinge_angle` | `a3b53fe2753556e1` / `12624a343a91e372` | 5 float，`[0]` 角度 |
| `flip_status` | `191b30ebf1194817` / `95682728dd533367` | 12 float，`[1]` = 179.0 |
| `device_orient` | `eef435f9287516a7` / `a1964b507cf7e3e0` | 16 float，恒定 `4,4,0…` |
| accel / gravity / gyro | — | 512/513/514 只回 inner 130（`INVALID_VALUE`/`NOT_SUPPORTED`） |

accel 的失败不是缺消息号：`libsnsapi` 全量枚举里没有任何 accel 专属 event/config，
样本就是标准 1025 + 3×float32；它失败是因为**属性表为空**（registry 被 SSR 清空），
配置在校验阶段就被拒。registry 修好之后需要用 `msgid 1` 复查 `attr 16`（stream_type）
再决定用 513（连续型，f4 是 fixed32 Hz）还是 514（on-change，f4 是 varint 微秒）。

## 8. 还没定的一条

`init.recovery.qcom.rc` 那两行要不要整段删。取决于 **ADSP 能不能不靠这一发自己起来**：

- `remoteproc1/state` 仍是 `running` → 那两行是「固件可见性 + 强制重下」的补丁，
  可以只删 `write … ssr 1`，保留 bind 与 `wait`；
- 变成 `offline` → 这一发同时兼着上电，得改成条件写法：

```sh
exec root root -- /system/bin/sh -c '[ "$(cat /sys/class/remoteproc/remoteproc1/state 2>/dev/null)" = "running" ] || echo 1 > /sys/kernel/boot_adsp/ssr'
```

副作用评估：recovery 不跑 audio HAL（那唯一的运行时写者 `audiohalservice_qti.rc`
的 `vendor.audio.ssr.trigger` 不生效），充电走 `charger_fstab` + 内核 `power_supply`，
温度走内核 thermal，**删这一发对它们没有依赖**；唯一的耦合就是 ADSP 自己的固件加载。

## 9. 已知的失败路径（别重试）

- 清空 `/mnt/vendor/persist/sensors/registry/registry` 再 SSR：注册表不回来，也不会写回文件；
- 停 `sscrpcd`／停 HAL／停 `qsap_sensors` 再 SSR：样本、注册表都不受影响，说明都不是提供者；
- 用 `bootmonitor.bootmode` 让 ADSP 按 normal 起：cmdline 只读，`/sys/kernel/boot_adsp`
  只有 `boot` 与 `ssr` 两个写节点，没有 bootmode 入口；
- 内核 IIO 直读：recovery 里没有 `i2c-dev.ko`／`spidev.ko`，`i2c-7` 的 `0x6a/0x6c` 是充电泵
  `sc8541`，`spi0/1/2` 没有 spidev。
