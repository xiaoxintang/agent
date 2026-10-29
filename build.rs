//! 从仓库内固定版本的 proto 生成描述文件，不在构建期间下载上游代码。

use prost::Message;
use std::{env, fs, path::Path};

fn collect(directory: &Path, files: &mut Vec<std::path::PathBuf>) {
    for entry in fs::read_dir(directory).expect("read vendored proto directory") {
        let path = entry.expect("read proto entry").path();
        if path.is_dir() {
            collect(&path, files);
        } else if path
            .extension()
            .is_some_and(|extension| extension == "proto")
        {
            files.push(path);
        }
    }
}

fn main() {
    println!("cargo:rerun-if-changed=proto");
    let mut files = Vec::new();
    collect(Path::new("proto"), &mut files);
    files.sort();
    // protox 直接解析 proto，无需系统 protoc；运行时编解码和 OpenAPI 共用此描述集。
    let descriptors = protox::compile(files, ["proto"]).expect("compile pinned Xray descriptors");
    let output = Path::new(&env::var_os("OUT_DIR").expect("OUT_DIR")).join("xray.bin");
    fs::write(output, descriptors.encode_to_vec()).expect("write Xray descriptors");
}
