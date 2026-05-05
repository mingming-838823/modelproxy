fn main() {
    let target = std::env::var("TARGET").unwrap_or_default();
    
    if target.contains("windows") {
        let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
        let rc_path = std::path::Path::new(&manifest_dir).join("modelproxy-gui.rc");
        let res_path = std::path::Path::new(&manifest_dir).join("modelproxy-gui.res");
        
        if target.contains("x86_64") && target.contains("gnu") {
            if res_path.exists() {
                println!("cargo:rustc-link-arg-bin=modelproxy-gui={}", res_path.display());
                println!("cargo:warning=Using pre-compiled Windows resource: {:?}", res_path);
            } else {
                let windres_path = if cfg!(target_os = "windows") {
                    "d:/msys64/mingw64/bin/windres.exe"
                } else {
                    "x86_64-w64-mingw32-windres"
                };
                let status = std::process::Command::new(windres_path)
                    .arg(&rc_path)
                    .arg("-O")
                    .arg("coff")
                    .arg("-o")
                    .arg(&res_path)
                    .status()
                    .expect("Failed to run windres");
                
                if status.success() {
                    println!("cargo:rustc-link-arg-bin=modelproxy-gui={}", res_path.display());
                    println!("cargo:warning=Compiled Windows resource successfully");
                } else {
                    println!("cargo:warning=Failed to compile Windows resource");
                }
            }
        }
    }
}
