//! Windows: puts the Alfadoc icon and version into the .exe. A failure here only means the exe has
//! no icon; it never stops the build.

fn main() {
    println!("cargo:rerun-if-changed=../../packaging/icon/alfadoc.ico");
    #[cfg(windows)]
    {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("../../packaging/icon/alfadoc.ico");
        res.set("ProductName", "Alfadoc");
        res.set("FileDescription", "Alfadoc: documents, spreadsheets and presentations in one app");
        if let Err(e) = res.compile() {
            println!("cargo:warning=could not embed the Windows icon: {e}");
        }
    }
}
