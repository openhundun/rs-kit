# rs-kit

| 路径                 | 说明                                                                                                   |
| -------------------- | ------------------------------------------------------------------------------------------------------ |
| `Cargo.toml`         | Cargo workspace 清单（`resolver = "3"`，共享依赖与 release profile）                                   |
| `source/cargo-flow/` | cargo 子命令 `cargo flow`：Cargo.toml 结构归一（点键展开为值嵌套，可选 features / default-features / 版本号归一） |
