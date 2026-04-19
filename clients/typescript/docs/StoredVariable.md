
# StoredVariable

A stored variable — a plaintext key-value pair for template substitution in deployment specs. Variables are NOT encrypted (unlike secrets). They live in their own storage, separate from the encrypted secrets store.  Variables can be global (`scope = None`) or project-scoped (`scope = Some(project_id)`).

## Properties

Name | Type
------------ | -------------
`createdAt` | string
`id` | string
`name` | string
`scope` | string
`updatedAt` | string
`value` | string

## Example

```typescript
import type { StoredVariable } from '@zlayer/client'

// TODO: Update the object below with actual values
const example = {
  "createdAt": 2026-04-15T12:00:00Z,
  "id": null,
  "name": null,
  "scope": null,
  "updatedAt": 2026-04-15T12:00:00Z,
  "value": null,
} satisfies StoredVariable

console.log(example)

// Convert the instance to a JSON string
const exampleJSON: string = JSON.stringify(example)
console.log(exampleJSON)

// Parse the JSON string back to an object
const exampleParsed = JSON.parse(exampleJSON) as StoredVariable
console.log(exampleParsed)
```

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


