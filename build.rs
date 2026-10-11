// Rebuild when the ingestion service compiled into the binary changes (read with option_env!).

/** Declare the environment variables the build depends on
 * Input
    - None
 * Output
    - None
*/
fn main() {
    println!("cargo:rerun-if-env-changed=CRANE_DEFAULT_INGEST_URL");
}
