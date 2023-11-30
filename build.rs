use std::path::PathBuf;

fn main() {
    gen_bindings();
}

fn gen_bindings() {
    let bindings = bindgen::Builder::default()
        .header("src/shm_datastructs/wrapper.h")
        .clang_arg("-I./src/shm_datastructs/LookingGlass/common/include/common")
        .parse_callbacks(Box::new(bindgen::CargoCallbacks))
        .generate()
        .expect("Unable to generate bindings");

    let out_path = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    bindings
        .write_to_file(out_path.join("common_bindings.rs"))
        .expect("Unable to write bindings to file");
    println!("cargo:rerun-if-changed=wrapper.h");
}
