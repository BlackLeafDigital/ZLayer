# ZLayer TypeScript SDK

The ZLayer Plugin Development Kit (PDK) for TypeScript enables you to build WebAssembly plugins that run in the ZLayer runtime.

## Requirements

- Node.js 20.0.0 or later
- npm or pnpm

## Installation

```bash
npm install @zlayer/sdk
```

Or with pnpm:

```bash
pnpm add @zlayer/sdk
```

## Quick Start

1. **Initialize a new plugin project:**

```bash
mkdir my-plugin && cd my-plugin
npm init -y
npm install @zlayer/sdk
```

2. **Create your plugin entry point:**

```typescript
// src/index.ts
import { VERSION } from '@zlayer/sdk';

console.log(`Using ZLayer SDK v${VERSION}`);

// Your plugin implementation here
```

3. **Build to WebAssembly:**

```bash
npm run build
```

## Project Structure

```
my-plugin/
├── src/
│   └── index.ts          # Plugin entry point
├── dist/                  # Compiled JavaScript output
├── package.json
└── tsconfig.json
```

## Generating WIT Bindings

The SDK uses [jco](https://github.com/bytecodealliance/jco) to generate TypeScript bindings from WIT (WebAssembly Interface Types) definitions.

To regenerate bindings from the ZLayer WIT definitions:

```bash
npm run generate
```

This will read the WIT files from `../wit/zlayer` and output TypeScript bindings to `src/bindings/`.

## Available Scripts

| Script | Description |
|--------|-------------|
| `npm run build` | Compile TypeScript to JavaScript |
| `npm run generate` | Generate WIT bindings using jco |
| `npm run clean` | Remove the dist directory |
| `npm run test` | Run tests with Vitest |
| `npm run lint` | Lint source files with ESLint |
| `npm run format` | Format source files with Prettier |

## Configuration

### TypeScript

The SDK is configured for ES2022 with NodeNext module resolution. See `tsconfig.json` for full configuration.

### ESLint

ESLint is configured with TypeScript support. See `.eslintrc.json` for rules.

### Prettier

Prettier is configured for consistent code formatting. See `.prettierrc` for options.

## Examples

Check the `examples/` directory for sample plugin implementations.

## Host Capabilities

Plugins built with this SDK can access ZLayer host capabilities through the generated WIT bindings:

- **HTTP Requests** - Make outbound HTTP requests
- **Key-Value Storage** - Persist data across plugin invocations
- **Logging** - Structured logging to the host
- **Configuration** - Access plugin configuration
- **Events** - Subscribe to and emit events

## Building for Production

For production builds, ensure you:

1. Run the full build process:
   ```bash
   npm run clean && npm run build
   ```

2. Test your plugin:
   ```bash
   npm run test
   ```

3. Lint and format:
   ```bash
   npm run lint && npm run format
   ```

## License

MIT
