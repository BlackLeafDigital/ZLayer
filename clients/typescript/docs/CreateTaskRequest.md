
# CreateTaskRequest

Body for `POST /api/v1/tasks`.

## Properties

Name | Type
------------ | -------------
`body` | string
`kind` | [TaskKind](TaskKind.md)
`name` | string
`projectId` | string

## Example

```typescript
import type { CreateTaskRequest } from '@zlayer/client'

// TODO: Update the object below with actual values
const example = {
  "body": null,
  "kind": null,
  "name": null,
  "projectId": null,
} satisfies CreateTaskRequest

console.log(example)

// Convert the instance to a JSON string
const exampleJSON: string = JSON.stringify(example)
console.log(exampleJSON)

// Parse the JSON string back to an object
const exampleParsed = JSON.parse(exampleJSON) as CreateTaskRequest
console.log(exampleParsed)
```

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


