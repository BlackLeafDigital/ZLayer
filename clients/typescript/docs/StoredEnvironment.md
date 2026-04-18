
# StoredEnvironment

A deployment/runtime environment (e.g. \"dev\", \"staging\", \"prod\").  Each environment is an isolated namespace for secrets and, later, deployments. Optionally belongs to a `Project` (added in Phase 5) — when `project_id` is `None`, the environment is global.

## Properties

Name | Type
------------ | -------------
`createdAt` | string
`description` | string
`id` | string
`name` | string
`projectId` | string
`updatedAt` | string

## Example

```typescript
import type { StoredEnvironment } from '@zlayer/client'

// TODO: Update the object below with actual values
const example = {
  "createdAt": 2026-04-15T12:00:00Z,
  "description": null,
  "id": null,
  "name": null,
  "projectId": null,
  "updatedAt": 2026-04-15T12:00:00Z,
} satisfies StoredEnvironment

console.log(example)

// Convert the instance to a JSON string
const exampleJSON: string = JSON.stringify(example)
console.log(exampleJSON)

// Parse the JSON string back to an object
const exampleParsed = JSON.parse(exampleJSON) as StoredEnvironment
console.log(exampleParsed)
```

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


