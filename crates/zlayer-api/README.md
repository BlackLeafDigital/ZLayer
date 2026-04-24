# zlayer-api

REST API server for ZLayer container orchestration. Implements the HTTP endpoints that the `zlayer` daemon exposes for deployments, services, jobs, cron resources, image builds, and streaming logs.

This is an internal crate and is not published to crates.io. Consumers should use the public [`zlayer`](https://github.com/zachhandley/ZLayer) CLI, or one of the generated OpenAPI clients:

- TypeScript: [`clients/typescript`](../../clients/typescript)
- Python: [`crates/zlayer-py`](../zlayer-py)

## License

MIT - See [LICENSE](../../LICENSE) for details.
