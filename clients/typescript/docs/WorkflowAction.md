
# WorkflowAction

The action a workflow step performs.

## Properties

Name | Type
------------ | -------------
`taskId` | string
`type` | string
`projectId` | string
`syncId` | string

## Example

```typescript
import type { WorkflowAction } from '@zlayer/client'

// TODO: Update the object below with actual values
const example = {
  "taskId": null,
  "type": null,
  "projectId": null,
  "syncId": null,
} satisfies WorkflowAction

console.log(example)

// Convert the instance to a JSON string
const exampleJSON: string = JSON.stringify(example)
console.log(exampleJSON)

// Parse the JSON string back to an object
const exampleParsed = JSON.parse(exampleJSON) as WorkflowAction
console.log(exampleParsed)
```

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


