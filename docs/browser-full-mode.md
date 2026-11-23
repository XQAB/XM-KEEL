# 完整模式：让浏览器读得动脚本画出来的页面

这一页记「浏览器要能正常浏览绝大多数网站」这件事怎么落地的：变量在哪、
取舍是什么、真机上量到了什么、哪条路已经证明走不通。

## 一、为什么不是纯文本就够

`web::parse` 读的是服务器给的那份 HTML。现在的页面大半是脚本把正文拼出来的，
那份 HTML 里没有正文，只有一串 `<script>`。所以纯文本路线在新闻站、文档站、
GitHub 这类页面上会读出一片空。

出路不是自己写 JS 引擎，而是让一个真的浏览器把页面画出来，再把它画完之后
的 DOM 交回来——DOM 是 HTML，`parse` 认得。这个浏览器是 aurora 做的那份
外置 arm64 Chromium（`browser-v0.3.4-aurora` 的 `chromium.zip`），它本身是
一个完整的 glibc rootfs，用 `chroot` 就能在 recovery 里跑起来。

## 二、插件放哪、代码怎么走

插件目录固定三个候选，按出现顺序取第一个：

```
/data/local/Skyward/plugins/chromium      # 下载落点；真 ext4，权限位算数
/data/media/0/Skyward/plugins/chromium     # 插件 README 里的正式位置，留给手工拷贝
/sdcard/Skyward/plugins/chromium           # 同一目录在运行系统里的另一个名字
```

第一顺位从 `/data/media/0` 改成 `/data/local`，理由是「能跑起来」这四个字：
`/data/media` 是 FUSE（`/sdcard` 就是它），`chmod +x` 在那一层基本是空操作，
用给的 mode 去 `set_permissions` 也不一定生效。插件是一整套 glibc rootfs，
里面 `usr/lib/chromium/chromium`、`chrome_crashpad_handler`、一堆 `*.so`
都要可执行，靠 FUSE 那层赌它放行不如直接落在 `/data` 自己的 ext4 上。
`/data/media/0` 那份仍然认，是为了「人用文件管理器手动拷进去」这条路不断。

判据只有一个文件：`usr/lib/chromium/chromium` 在不在。

代码分三处：

- `crates/keel/src/full.rs`：`plugin()` 找目录，`ready()` 把插件自己的
  `tmp/`、`tmp/given/`、`tmp/xdg/` 建好、把宿主机的 `/dev`、`/proc`、`/sys`
  用 `libc::mount` 绑进插件（不用外部 `mount` 命令，recovery 里没有），并把
  宿主机的 `/etc/resolv.conf` 抄给插件；`dom()` 用 `Command::pre_exec`
  做 `chroot` + `chdir("/")` + `setsid`，`env_clear` 之后只设
  `PATH`/`HOME`/`TMPDIR`/`XDG_RUNTIME_DIR`/`XDG_CACHE_HOME`/`XDG_CONFIG_HOME`，
  跑 `--headless=new --dump-dom --virtual-time-budget=5000`，30 秒没回来就
  `kill(-pid, SIGKILL)` 整组收掉。**给插件的一切路径都必须是 chroot 里的
  `/tmp`、`/usr` 这类名字，不能带宿主机前缀**：`--user-data-dir` 传的是
  `/tmp/given`（`ready()` 早就把它建好了），传宿主机绝对路径的话，进 chroot
  之后那个目录根本不存在，Chromium 建不了 profile。它的 stderr 也不吞：
  落到 `<插件目录>/tmp/plugin.err`（在 chroot 前用 `File::create` 打开，
  描述符跟着 `pre_exec` 过去），起不来时最后三行会被带进报错里。
- `crates/keel-net/src/web.rs`：`pub fn rendered(agent, url, html)` 把
  拿回来的 DOM 交给 `parse`，再 `fetch_images`，得到和普通读取完全一样的
  `Page`。UI 侧不知道也不关心这一页是怎么来的。
- `crates/keel/src/updates.rs` 的 `Task::Browse`：有插件就先 `full::dom`，
  成功走 `web::rendered`；插件不在、或者插件起不来（`kmsg` 记一条），退回
  `web::open`。**所以插件是可选的加速项，不是硬依赖**：没装插件的机器，
  浏览器还是原来的纯文本，只是读不动脚本页。

这条路由是自动的，没做用户开关。理由：多一个开关就多一个「为什么这页是空的」
的解释成本，而两种模式对用户来说都是「把这一页读出来」，能读得更多的那条
就该默认走；插件坏了自动退回原文，也不需要用户去关掉什么。

## 三、真机上量到的

bixi（`507d820c`，Android SDK 37，arm64-v8a，root 可用）实测：

| 事 | 数 |
| --- | --- |
| `chromium.zip` | 457 339 582 B，sha256 `9e6df852…4ea2`（与 release digest 一致） |
| 解开后 rootfs | 1.0 GB，`bin/`、`lib/`、`usr/lib/chromium/`、`run.sh` |
| 推到手机 | 9.9 MB/s，44 秒 |
| `example.com` 截图 | 412×915 PNG，62 836 B |
| `example.com` 的 DOM | 2 375 B，`<p lang="zh">该域名仅用于文档示例…</p>` 在 |

`chroot` 起得来，需要的 env 一个都不能少：

```sh
PATH=/bin:/usr/bin:/usr/sbin
HOME=/root
TMPDIR=/tmp
XDG_RUNTIME_DIR=/tmp/xdg      # 少了这条 Chromium 直接 abort：
                              # Failed to create socket directory
XDG_CACHE_HOME=/tmp/cache
XDG_CONFIG_HOME=/tmp/config
```

并且 `tmp/xdg`、`tmp/given` 要 `chmod 700`；插件根下必须自己 `mkdir` 出
`proc`、`sys`、`dev/shm` 再 `mount --bind`，rootfs 里本来没有这三个目录。

## 四、走不通的路（别再试）

1. **不要用 `cp -a` 把 1 GB 的 rootfs 从 `/data/local/tmp` 拷到 `/data/media/0`。**
   拷完内容是坏的：`bin/busybox` 的 md5 是 `46cf0c7d…`，源是 `bfa28331…`，
   头 4 字节变成 `00 00 00 00`，`chroot` 报 `Exec format error`。
   正解是把 zip 拷过去，用设备自带的 `unzip` 就地解开，解完 md5 与源一致
   （本轮就是这么修好的）。
2. **GitHub 直连下 release 资产不行。** HEAD/Range 都 `RemoteDisconnected`，
   `ghfast.top`/`ghproxy.net` 时好时坏。`https://gh-proxy.com/` + 原地址实测
   1 032 KB/s，436 MiB 七分钟下完。
3. **不要在 recovery 外直接调 `fc-match` 当判据。** rootfs 里的 `fc-*` 是
   glibc 动态程序，得用 `usr/lib/aarch64-linux-gnu/ld-linux-aarch64.so.1`
   显式带起来；直接 `fc-match` 报 `not found` 不是字体坏了。

## 五、字体（中文/Arabic）

rootfs 自带 4 个 Noto CJK（`NotoSansCJK-{Regular,Bold}.ttc`、
`NotoSerifCJK-{Regular,Bold}.ttc`，19–27 MB，`ttcf` 魔数正常），
`fc-match 'sans:lang=zh'` 映射到 `NotoSansCJK-Regular.ttc`，
`fc-list :lang=zh` 正常列出 SC/TC/JP。

真机结论：**本地页四种 `font-family`（默认 sans、显式 SC、显式 JP、serif）
都正常渲染中文**，`ft.png` 16 121 B 可复现；跨域页（`example.com`）的中文与
Arabic 一开始是豆腐块，但同一份 rootfs 之后重跑就正常了——第一张截图很可能
发生在 fontconfig 缓存（`var/cache/fontconfig/…cache-9`）刚建、还没被读到的
那一刻。所以豆腐块不是字体缺失，是首次运行的缓存时序。

## 六、还没验证的

- **recovery 里 `/data/media/0` 能不能 `exec`。** Android 侧实测能（截图就是
  从正式路径跑出来的），但 recovery 是裸挂 `/data`，没有 `sdcardfs`/FUSE
  那一层，只能等真机验。若不能，`full::plugin()` 会找不到插件，浏览器自动
  退回纯文本——不会坏，只是没这份能力。
- **一页要多久。** 单次 Chromium 冷启动在这台机器上是秒级，`BUDGET` 给 30 秒
  是上限不是常态；没有在完整模式下连续翻页压过。
- **图片真渲染。** DOM 里的 `<img>` 现在是 `Block::Image{alt,url,data,…}`，
  UI 侧仍只画一行「［图片］alt」；要真画图得加 PNG/JPEG 解码和
  `Canvas` 的任意尺寸 blit，还没做。

## 七、插件从哪里来：更新页上的「完整浏览器（可选）」

插件不随 recovery 一起打包，它太大（zip 457 339 582 B，解开 1.0 GB），
而且不是每个用户都需要。它放在「更新 → 在线更新」下面，由用户自己决定：

- **未装**：一行「下载完整浏览器插件」＋副标题（约 436 MB、脚本页也能读）。
- **下载中**：进度卡（已下载 / 全部、速率）＋「停止」。
- **已装**：一行「已安装 · 占用 X」＋一行红色「删除插件」。

下载按「已选镜像 → 其余镜像 → github.com 直连」的顺序挨个试：`MIRRORS`
里的每个前缀都进候选表（选中的排第一，空前缀直连排最后），不是只试一条。
理由是实测过 `gh-proxy` 这一层会给回截断或不同内容的文件，436 MB 一次下完
才发现 SHA-256 不对（`Failure::Damaged`），如果候选表只有一条就直接判死，
永远回不到直连。zip 落到 `<插件目录>/chromium.zip`，校验 SHA-256
`9e6df85255386eb962bbe6b1ad5e0d2505c7b5ba02a9d7d89d3d20a778f14ea2`
（与 release 的 digest 一致，真机实测 457 339 582 B），然后用
`zip::ZipArchive` 就地解到 `/data/local/Skyward/plugins/chromium`，
按 `unix_mode()` 还原权限（还原失败会带着文件名报错，不再静默吞掉——Chromium
没有 +x 就 exec 不了，这正是「下载完了还是不显示已安装」的一种真因），
越界名一律拒绝，解完把 zip 删掉。

**为什么不自动装、也不默认装**：一份 436 MB 的下载、1 GB 的落盘，只有
用户自己说要才值得花；不装的机器行为与以前完全一样，一行代码都不变。

**为什么下载路径和 `plugin_recopy.sh` 的做法一致**：`adb push` 上去的 zip
用设备自带 `unzip` 解是唯一验证过的路径；`cp -a` 从 `/data/local/tmp` 拷到
`/data/media/0` 会把 `bin/busybox` 拷坏（md5 对不上，首 4 字节全 0，
chroot 报 `Exec format error`）。所以 zip 和插件目录现在都落在 `/data/local`
下，解压仍然不跨文件系统（`/data/local` 就在 `/data` 上）。

**删除**是 `remove_dir_all` + `remove_file`，三处候选目录都删一遍，
NotFound 当成功（重复点不会报错）。删完 `full::plugin()` 返回 `None`，
浏览器自动退回纯文本，不需要重启。

**已知未验**：recovery 里这条下载路径没有真机跑过（Android 侧跑过等价流程），
首次真机点「下载」时请一并看 `/proc/kmsg` 里的 `taking the full browser into …`。
