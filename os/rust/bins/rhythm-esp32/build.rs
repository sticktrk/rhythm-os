fn main() {
    println!("cargo:rerun-if-changed=partitions.csv");
    embuild::espidf::sysenv::output();
}
