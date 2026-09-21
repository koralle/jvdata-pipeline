//! jvlink-bridge.exe — Wine/Windows 内で動く JV-Link (COM/ActiveX) の
//! 薄いラッパー。`JVGets` が返す raw bytes を framed binary で stdout に流す。
//!
//!   jvlink-bridge.exe ping
//!   jvlink-bridge.exe fetch --dataspec RACE --from FROMTIME --option 4
//!                           [--skip-to-file FILENAME]
//!
//! JV-Link の知識はこのファイルに閉じ込める。ドメイン知識は持たない。
//! unsafe はこのファイルと protocol エンコーダに限定する。

#[cfg(not(windows))]
fn main() {
    eprintln!("jvlink-bridge runs only under Windows (wine)");
    std::process::exit(2);
}

#[cfg(windows)]
fn main() {
    std::process::exit(win::entry());
}

#[cfg(windows)]
mod win {
    use bridge::protocol::Frame;
    use std::ffi::c_void;
    use std::io::{BufWriter, Write};
    use std::mem::ManuallyDrop;
    use windows::Win32::System::Com::{
        CLSCTX_INPROC_SERVER, CLSCTX_LOCAL_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance,
        CoInitializeEx, CoUninitialize, DISPATCH_METHOD, DISPATCH_PROPERTYGET, DISPPARAMS,
        EXCEPINFO, IDispatch, SAFEARRAY,
    };
    use windows::Win32::System::Ole::{SafeArrayAccessData, SafeArrayUnaccessData};
    use windows::Win32::System::Variant::{
        VARENUM, VARIANT, VT_ARRAY, VT_BSTR, VT_BYREF, VT_I4, VT_UI1,
    };
    use windows::core::{BSTR, GUID, PCWSTR};

    /// JVDTLab ActiveX コントロールの CLSID (SDK jvlink.h)
    const CLSID_JVDTLAB: GUID = GUID::from_u128(0x2AB1774D_0C41_11D7_916F_0003479BEB3F);

    // dispid (SDK sample jvlink.h の InvokeHelper 第 1 引数)
    const D_JVINIT: i32 = 0x4;
    const D_JVCLOSE: i32 = 0x5;
    const D_JVOPEN: i32 = 0x7;
    const D_JVSTATUS: i32 = 0x8;
    const D_JVGETS: i32 = 0x16;
    const D_JVSKIP: i32 = 0x13;
    const D_JVSETSAVEPATH: i32 = 0x1;
    const D_JVSETSERVICEKEY: i32 = 0xd;
    const D_JVSETSAVEFLAG: i32 = 0xf;
    /// m_CurrentFileTimestamp (仕様書記載。ラッパーには無いので名前解決→失敗時 fallback)
    const D_CURFILETS: i32 = 0x14;

    /// JVGets のバッファサイズ (公式サンプルと同じ)
    const BUFSIZE: i32 = 110_000;

    struct JvErr(String);
    type R<T> = Result<T, JvErr>;
    fn err<T>(m: impl Into<String>) -> R<T> {
        Err(JvErr(m.into()))
    }

    fn emit(w: &mut impl Write, f: &Frame) -> R<()> {
        w.write_all(&f.encode()).map_err(|e| JvErr(e.to_string()))?;
        w.flush().map_err(|e| JvErr(e.to_string()))
    }

    /// VARIANT ヘルパー群。windows クレートの VARIANT は
    /// Anonymous: VARIANT_0 (union) → Anonymous: ManuallyDrop<VARIANT_0_0>
    /// → Anonymous: VARIANT_0_0_0 (union)。
    fn v_i4(x: i32) -> VARIANT {
        let mut v = VARIANT::default();
        // SAFETY: union フィールドへ vt と一致する値を書き込むだけ。
        unsafe {
            let inner = &mut v.Anonymous.Anonymous;
            inner.vt = VT_I4;
            inner.Anonymous.lVal = x;
        }
        v
    }

    /// [in] BSTR 引数。BSTR の所有権ごと VARIANT に渡す。
    ///
    /// windows クレートの VARIANT は Drop で VariantClear を呼ぶ。vt=VT_BSTR
    /// なら中のポインタを SysFreeString するので、解放はそこで 1 度だけ
    /// 起きる。したがって借用 (&BSTR) を指す VARIANT を作ると、VARIANT の
    /// drop と BSTR 自身の drop で同じ割り当てを 2 度解放してしまう。
    /// 所有権を移し、元の BSTR の drop を forget で抑制して 1 度にする。
    fn v_bstr(s: BSTR) -> VARIANT {
        let mut v = VARIANT::default();
        // SAFETY: union フィールドへ vt と一致する値を書き込むだけ。
        // bstrVal は s と同じ割り当てを指すが、所有権は VARIANT に移った。
        unsafe {
            let inner = &mut v.Anonymous.Anonymous;
            inner.vt = VT_BSTR;
            inner.Anonymous.bstrVal = ManuallyDrop::new(BSTR::from_raw(s.as_ptr()));
        }
        std::mem::forget(s);
        v
    }

    fn v_out_i4(p: &mut i32) -> VARIANT {
        let mut v = VARIANT::default();
        // SAFETY: [out] i32 へのポインタを渡す。p は Invoke 呼び出し中
        // 生きている (&mut の呼び出し元変数)。
        unsafe {
            let inner = &mut v.Anonymous.Anonymous;
            inner.vt = VARENUM(VT_I4.0 | VT_BYREF.0);
            inner.Anonymous.plVal = p;
        }
        v
    }

    fn v_out_bstr(p: &mut BSTR) -> VARIANT {
        let mut v = VARIANT::default();
        // SAFETY: [out] BSTR へのポインタを渡す。p は Invoke 呼び出し中
        // 生きている。callee が書き込んだ BSTR は p の Drop で解放される。
        unsafe {
            let inner = &mut v.Anonymous.Anonymous;
            inner.vt = VARENUM(VT_BSTR.0 | VT_BYREF.0);
            inner.Anonymous.pbstrVal = p as *mut BSTR as _;
        }
        v
    }

    fn v_out_variant(p: &mut VARIANT) -> VARIANT {
        let mut v = VARIANT::default();
        // SAFETY: [out] VARIANT へのポインタを渡す。p は Invoke 呼び出し中
        // 生きている。
        unsafe {
            let inner = &mut v.Anonymous.Anonymous;
            inner.vt = VARENUM(VT_BYREF.0 | 0x000C /* VT_VARIANT */);
            inner.Anonymous.pvarVal = p;
        }
        v
    }

    struct Jv {
        disp: IDispatch,
        /// m_CurrentFileTimestamp の dispid (0 で未解決)
        curts_dispid: i32,
    }

    impl Jv {
        fn create() -> R<Self> {
            // SAFETY: CoInitializeEx 済みのスレッドで呼ぶ。成功時は所有権の
            // ある IDispatch が返る。
            let disp: IDispatch = unsafe {
                CoCreateInstance(
                    &CLSID_JVDTLAB,
                    None,
                    CLSCTX_INPROC_SERVER | CLSCTX_LOCAL_SERVER,
                )
                .map_err(|e| JvErr(format!("CoCreateInstance(JVDTLab): {e}")))?
            };
            // m_CurrentFileTimestamp はラッパー生成されていないので名前解決する
            let curts_dispid =
                dispid_by_name(&disp, "m_CurrentFileTimestamp").unwrap_or(D_CURFILETS);
            Ok(Self { disp, curts_dispid })
        }

        /// dispid を名前で引く (成功しなければ fallback を呼び出し側で)。
        fn invoke(&self, id: i32, args: &mut [VARIANT], method: bool) -> R<VARIANT> {
            let mut result = VARIANT::default();
            let params = DISPPARAMS {
                rgvarg: if args.is_empty() {
                    std::ptr::null_mut()
                } else {
                    args.as_mut_ptr()
                },
                rgdispidNamedArgs: std::ptr::null_mut(),
                cArgs: args.len() as u32,
                cNamedArgs: 0,
            };
            let mut excep = EXCEPINFO::default();
            // SAFETY: rgvarg は cArgs 個の有効な VARIANT を指し、
            // result/excep は有効な out ポインタ。COM apartment は
            // 初期化済み。
            unsafe {
                self.disp
                    .Invoke(
                        id,
                        &GUID::zeroed(),
                        0x400, // LOCALE_USER_DEFAULT
                        if method {
                            DISPATCH_METHOD
                        } else {
                            DISPATCH_PROPERTYGET
                        },
                        &params,
                        Some(&mut result),
                        Some(&mut excep),
                        None,
                    )
                    .map_err(|e| JvErr(format!("invoke dispid={id}: {e}")))?;
            }
            Ok(result)
        }

        fn call_i4(&self, id: i32, args: &mut [VARIANT]) -> R<i32> {
            let r = self.invoke(id, args, true)?;
            // JV-Link のメソッド戻り値は VT_I4。vt を確認してから lVal を読む
            // (VARIANT の union は vt と無関係なフィールドを読むと不正値/UB)。
            if r.vt() != VT_I4 {
                return err(format!("invoke dispid={id}: unexpected vt {:#x}", r.vt().0));
            }
            // SAFETY: vt=VT_I4 確認済み。lVal を読む。
            Ok(unsafe { r.Anonymous.Anonymous.Anonymous.lVal })
        }

        /// propget → BSTR → String
        fn prop_bstr(&self, id: i32) -> R<String> {
            let r = self.invoke(id, &mut [], false)?;
            if r.vt() != VT_BSTR {
                return err(format!(
                    "propget dispid={id}: unexpected vt {:#x}",
                    r.vt().0
                ));
            }
            // SAFETY: vt=VT_BSTR 確認済み。bstrVal を読む。
            let s = unsafe { r.Anonymous.Anonymous.Anonymous.bstrVal.to_string() };
            Ok(s)
        }

        fn init(&self, sid: &str) -> R<i32> {
            self.call_i4(D_JVINIT, &mut [v_bstr(BSTR::from(sid))])
        }

        /// 利用キー (17桁) をレジストリに保存。戻り値 0=成功 / -100=失敗 / -101=登録済み
        fn set_service_key(&self, key: &str) -> R<i32> {
            self.call_i4(D_JVSETSERVICEKEY, &mut [v_bstr(BSTR::from(key))])
        }

        /// JV-Data の保存パス。0=成功 / -100=失敗 / -102=パス不正
        fn set_save_path(&self, path: &str) -> R<i32> {
            self.call_i4(D_JVSETSAVEPATH, &mut [v_bstr(BSTR::from(path))])
        }

        /// 取得データの保存フラグ (1=保存して蓄積, 0=読み捨て)
        fn set_save_flag(&self, flag: i32) -> R<i32> {
            self.call_i4(D_JVSETSAVEFLAG, &mut [v_i4(flag)])
        }

        fn close(&self) {
            let _ = self.call_i4(D_JVCLOSE, &mut []);
        }

        fn skip(&self) {
            let _ = self.invoke(D_JVSKIP, &mut [], true);
        }

        fn status(&self) -> R<i32> {
            self.call_i4(D_JVSTATUS, &mut [])
        }

        fn cur_file_ts(&self) -> String {
            self.prop_bstr(self.curts_dispid).unwrap_or_default()
        }

        fn open(&self, dataspec: &str, fromtime: &str, option: i32) -> R<(i32, i32, i32, String)> {
            let mut readcount: i32 = 0;
            let mut downloadcount: i32 = 0;
            let mut lastts = BSTR::default();
            // rgvarg は逆順: 最後の引数が先頭
            let mut args = [
                v_out_bstr(&mut lastts),
                v_out_i4(&mut downloadcount),
                v_out_i4(&mut readcount),
                v_i4(option),
                v_bstr(BSTR::from(fromtime)),
                v_bstr(BSTR::from(dataspec)),
            ];
            let ret = self.call_i4(D_JVOPEN, &mut args)?;
            Ok((ret, readcount, downloadcount, lastts.to_string()))
        }

        /// JVGets。戻り値は (return_code, filename, record bytes)。
        fn gets(&self) -> R<(i32, String, Vec<u8>)> {
            let mut inner = VARIANT::default(); // VT_EMPTY。JV-Link が array をセットする
            let mut fname = BSTR::default();
            let mut args = [
                v_out_bstr(&mut fname),    // filename (BSTR*)
                v_i4(BUFSIZE),             // size
                v_out_variant(&mut inner), // buff (VARIANT*)
            ];
            let ret = self.call_i4(D_JVGETS, &mut args)?;
            let mut data = Vec::new();
            if ret > 0 {
                // ret>0 のとき JV-Link は inner に BYTE SAFEARRAY
                // (VT_ARRAY|VT_UI1) を設定する。それ以外の vt は仕様外。
                // vt を確認しないと union の別フィールドをポインタとして
                // 読んでしまう。
                if inner.vt() != VARENUM(VT_ARRAY.0 | VT_UI1.0) {
                    return err(format!("JVGets: unexpected vt {:#x}", inner.vt().0));
                }
                // SAFETY: inner の所有権はこちらにある (out VARIANT 引数)。
                // parray は vt=VT_ARRAY|VT_UI1 確認済み。AccessData で得た
                // ポインタは ret バイト有効で、UnaccessData までに読み切る。
                // 配列の解放は inner の Drop (VariantClear) に任せる。
                // ここで SafeArrayDestroy すると drop 時に二重解放になる。
                unsafe {
                    let psa: *mut SAFEARRAY = inner.Anonymous.Anonymous.Anonymous.parray;
                    if psa.is_null() {
                        return err("JVGets: null SAFEARRAY");
                    }
                    let mut pv: *mut c_void = std::ptr::null_mut();
                    if SafeArrayAccessData(psa, &mut pv).is_err() {
                        return err("JVGets: SafeArrayAccessData failed");
                    }
                    data = std::slice::from_raw_parts(pv as *const u8, ret as usize).to_vec();
                    let _ = SafeArrayUnaccessData(psa);
                }
            }
            Ok((ret, fname.to_string(), data))
        }
    }

    fn dispid_by_name(disp: &IDispatch, name: &str) -> Option<i32> {
        let w: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
        let names = [PCWSTR(w.as_ptr())];
        let mut ids = [0i32];
        // SAFETY: names は NUL 終端 UTF-16 を指し、ids は 1 要素の
        // 有効な out 配列。w/names/ids は呼び出し中に生存する。
        unsafe {
            disp.GetIDsOfNames(&GUID::zeroed(), names.as_ptr(), 1, 0x400, ids.as_mut_ptr())
                .ok()?;
        }
        Some(ids[0])
    }

    struct Args {
        dataspec: String,
        fromtime: String,
        option: i32,
        skip_to_file: Option<String>,
    }

    fn parse_args(argv: &[String]) -> R<Args> {
        let mut a = Args {
            dataspec: String::new(),
            fromtime: String::new(),
            option: 1,
            skip_to_file: None,
        };
        let mut i = 0;
        while i < argv.len() {
            let v = argv.get(i + 1).cloned();
            match argv[i].as_str() {
                "--dataspec" => {
                    a.dataspec = v.ok_or_else(|| JvErr("--dataspec needs value".into()))?
                }
                "--from" => a.fromtime = v.ok_or_else(|| JvErr("--from needs value".into()))?,
                "--option" => {
                    a.option = v
                        .and_then(|s| s.parse().ok())
                        .ok_or_else(|| JvErr("--option needs int".into()))?
                }
                "--skip-to-file" => a.skip_to_file = v,
                other => return err(format!("unknown arg: {other}")),
            }
            i += 2;
        }
        Ok(a)
    }

    /// JVInit/JVOpen/JVGets/JVClose の lifecycle をまとめる。
    /// エラー・早期 return を含む全経路で JVClose が呼ばれるよう、
    /// close はこの関数だけが行う (fetch_inner は close しない)。
    fn fetch(a: &Args, out: &mut impl Write) -> R<()> {
        let sid = std::env::var("JVDATA_SID").unwrap_or_else(|_| "UNKNOWN".into());
        let jv = Jv::create()?;
        let r = match jv.init(&sid) {
            Ok(0) => fetch_inner(&jv, a, out),
            Ok(ret) => emit_err(out, ret, "JVInit"),
            Err(e) => Err(e),
        };
        jv.close();
        r
    }

    /// ダウンロード停滞検出。JVStatus の戻り値 (ダウンロード完了ファイル数)
    /// が進まないポーリングが limit 回続いたら失敗にする。
    /// limit は JVDATA_STALL_SECS (秒) で上書き可能。既定 600 秒。
    /// 500ms ポーリングなので回数 = 秒 × 2。
    struct Stall {
        last: i32,
        quiet: u32,
        limit: u32,
    }

    impl Stall {
        fn new() -> Self {
            let secs = std::env::var("JVDATA_STALL_SECS")
                .ok()
                .and_then(|v| v.parse::<u32>().ok())
                .unwrap_or(600);
            Self {
                last: -1,
                quiet: 0,
                limit: secs.saturating_mul(2).max(2),
            }
        }

        /// 進捗があれば quiet をリセット。無進捗が limit 回続いたら true。
        fn tick(&mut self, progress: i32) -> bool {
            if progress != self.last {
                self.last = progress;
                self.quiet = 0;
            } else {
                self.quiet += 1;
            }
            self.quiet > self.limit
        }
    }

    fn fetch_inner(jv: &Jv, a: &Args, out: &mut impl Write) -> R<()> {
        let (ret, read_count, download_count, lastts) =
            jv.open(&a.dataspec, &a.fromtime, a.option)?;
        // -1 = 該当データなし (インターフェース仕様書)。その window は
        // 「0 件で完了」なのでエラーではなく Done として返す。
        // (-201 は JVInit 未呼出しのエラー。ここでは JVInit 済みなので
        //   来ないが、仕様上の意味を取り違えないこと)
        if ret == -1 {
            return emit(out, &Frame::Done);
        }
        if ret != 0 {
            return emit_err(out, ret, "JVOpen");
        }
        emit(
            out,
            &Frame::OpenOk {
                read_count,
                download_count,
                last_file_timestamp: lastts,
            },
        )?;

        let mut stall = Stall::new();

        // ダウンロード完了を待つ (JVStatus が download_count に達するまで)
        if download_count > 0 {
            loop {
                let s = jv.status()?;
                if s < 0 {
                    return emit_err(out, s, "JVStatus");
                }
                emit(
                    out,
                    &Frame::Progress {
                        done: s,
                        total: download_count,
                    },
                )?;
                if s >= download_count {
                    break;
                }
                if stall.tick(s) {
                    return emit_err(out, -9903, "JVStatus: download stalled");
                }
                std::thread::sleep(std::time::Duration::from_millis(500));
            }
        }

        let mut cur_file = String::new();

        // 再開: 保持していたファイル名まで JVSkip (仕様書の再開手順)。
        // gets でファイル名を確かめてから skip する順にする:
        //   - 再開対象ファイル自身は再読する (raw_records の冪等キーで吸収される)
        //   - 対象が先頭ファイルでも取りこぼさない
        //   - skip したファイルのレコードは emit しない
        //     (前回 run で commit 済み。emit すると別ファイル名で重複して残る)
        if let Some(target) = &a.skip_to_file {
            loop {
                let (ret, fname, data) = jv.gets()?;
                if ret > 0 {
                    if fname == *target {
                        emit(
                            out,
                            &Frame::FileBegin {
                                name: fname.clone(),
                                timestamp: jv.cur_file_ts(),
                            },
                        )?;
                        emit(out, &Frame::Record(data))?;
                        cur_file = fname;
                        break;
                    }
                    jv.skip();
                } else if ret == -3 {
                    // ダウンロード中。JVStatus で進捗を見て、
                    // 停滞が続くようなら失敗にする (永久 wait しない)。
                    std::thread::sleep(std::time::Duration::from_millis(500));
                    let s = jv.status()?;
                    if s < 0 {
                        return emit_err(out, s, "JVStatus");
                    }
                    if stall.tick(s) {
                        return emit_err(out, -9903, "JVGets: download stalled");
                    }
                } else if ret == -1 {
                    // ファイル境界。まだ次のファイルがあるかもしれないので継続。
                } else if ret == 0 {
                    // 対象が見つからないまま末尾に達した。
                    // 続きを読んだふりをして done にすると欠損するので失敗にする。
                    return emit(
                        out,
                        &Frame::Error {
                            code: -9901,
                            message: format!("skip-to-file {target} not found in stream"),
                        },
                    );
                } else {
                    return emit_err(out, ret, "JVGets(skip)");
                }
            }
        }

        // 本読み出しループ
        loop {
            let (ret, fname, data) = jv.gets()?;
            if ret > 0 {
                if fname != cur_file {
                    if !cur_file.is_empty() {
                        emit(
                            out,
                            &Frame::FileEnd {
                                name: cur_file.clone(),
                            },
                        )?;
                    }
                    emit(
                        out,
                        &Frame::FileBegin {
                            name: fname.clone(),
                            timestamp: jv.cur_file_ts(),
                        },
                    )?;
                    cur_file = fname;
                }
                emit(out, &Frame::Record(data))?;
            } else if ret == -1 {
                // 物理ファイルの終わり
                if !cur_file.is_empty() {
                    emit(
                        out,
                        &Frame::FileEnd {
                            name: cur_file.clone(),
                        },
                    )?;
                    cur_file.clear();
                }
            } else if ret == 0 {
                break;
            } else if ret == -3 {
                // まだダウンロード中のファイルがある。停滞検出は上と同じ。
                std::thread::sleep(std::time::Duration::from_millis(500));
                let s = jv.status()?;
                if s < 0 {
                    return emit_err(out, s, "JVStatus");
                }
                if stall.tick(s) {
                    return emit_err(out, -9903, "JVGets: download stalled");
                }
            } else {
                return emit_err(out, ret, "JVGets");
            }
        }

        emit(out, &Frame::Done)
    }

    fn emit_err(out: &mut impl Write, code: i32, what: &str) -> R<()> {
        emit(
            out,
            &Frame::Error {
                code,
                message: format!("{what} returned {code}"),
            },
        )
    }

    pub fn entry() -> i32 {
        let argv: Vec<String> = std::env::args().skip(1).collect();
        let mut out = BufWriter::new(std::io::stdout());
        let r = (|| -> R<()> {
            match argv.first().map(String::as_str) {
                Some("ping") => {
                    // COM 起動と JVInit までの疎通確認
                    // SAFETY: メインスレッドで 1 度だけ呼ぶ。終了時に
                    // CoUninitialize を対で呼ぶ。
                    unsafe {
                        CoInitializeEx(None, COINIT_APARTMENTTHREADED)
                            .ok()
                            .map_err(|e| JvErr(format!("CoInitializeEx: {e}")))?;
                    }
                    let jv = Jv::create()?;
                    let sid = std::env::var("JVDATA_SID").unwrap_or_else(|_| "UNKNOWN".into());
                    let init_ret = jv.init(&sid);
                    jv.close();
                    let ret = init_ret?;
                    eprintln!("JVInit -> {ret}");
                    if ret != 0 {
                        return err(format!("JVInit={ret}"));
                    }
                    Ok(())
                }
                Some("fetch") => {
                    // SAFETY: メインスレッドで 1 度だけ呼ぶ。終了時に
                    // CoUninitialize を対で呼ぶ。
                    unsafe {
                        CoInitializeEx(None, COINIT_APARTMENTTHREADED)
                            .ok()
                            .map_err(|e| JvErr(format!("CoInitializeEx: {e}")))?;
                    }
                    let a = parse_args(&argv[1..])?;
                    fetch(&a, &mut out)
                }
                Some("set-key") => {
                    // jvlink-bridge.exe set-key <17桁利用キー>
                    // 利用キー + 保存パス + 保存フラグをレジストリに書く。
                    // (JVSetUIProperties のダイアログを noVNC で開かずに済ませる)
                    // SAFETY: メインスレッドで 1 度だけ呼ぶ。終了時に
                    // CoUninitialize を対で呼ぶ。
                    unsafe {
                        CoInitializeEx(None, COINIT_APARTMENTTHREADED)
                            .ok()
                            .map_err(|e| JvErr(format!("CoInitializeEx: {e}")))?;
                    }
                    let key = argv
                        .get(1)
                        .ok_or_else(|| JvErr("usage: set-key <servicekey>".into()))?;
                    let jv = Jv::create()?;
                    let sid = std::env::var("JVDATA_SID").unwrap_or_else(|_| "UNKNOWN".into());
                    let results = (|| -> R<(i32, i32, i32)> {
                        let _ = jv.init(&sid);
                        let r1 = jv.set_service_key(key)?;
                        // 保存パスが存在しないと JVOpen が -211 になるので既定を入れる
                        let r2 = jv.set_save_path("C:\\jvdata")?;
                        let r3 = jv.set_save_flag(1)?;
                        Ok((r1, r2, r3))
                    })();
                    jv.close();
                    let (r1, r2, r3) = results?;
                    eprintln!("JVSetServiceKey -> {r1}");
                    eprintln!("JVSetSavePath -> {r2}");
                    eprintln!("JVSetSaveFlag -> {r3}");
                    if r1 != 0 && r1 != -101 {
                        return err(format!("JVSetServiceKey={r1}"));
                    }
                    Ok(())
                }
                _ => err(
                    "usage: jvlink-bridge.exe ping | set-key KEY | fetch --dataspec X --from T --option N [--skip-to-file F]",
                ),
            }
        })();
        let _ = out.flush();
        // SAFETY: CoInitializeEx と対で、スレッド終了前に 1 度だけ呼ぶ。
        unsafe { CoUninitialize() };
        match r {
            Ok(()) => 0,
            Err(e) => {
                // Error frame を可能なら出す (stdout がまだ生きていれば)
                let mut so = std::io::stdout();
                let _ = emit(
                    &mut so,
                    &Frame::Error {
                        code: -9999,
                        message: e.0.clone(),
                    },
                );
                eprintln!("jvlink-bridge: {}", e.0);
                1
            }
        }
    }
}
