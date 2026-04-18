
# WorkflowActionOneOf

Execute a task by id.

## Properties

Name | Type
------------ | -------------
`taskId` | string
`type` | string

## Example

```typescript
import type { WorkflowActionOneOf } from '@zlayer/client'

// TODO: Update the object below with actual values
const example = {
  "taskId": null,
  "type": null,
} satisfies WorkflowActionOneOf

console.log(example)

// Convert the instance to a JSON string
const exampleJSON: string = JSON.stringify(example)
console.log(exampleJSON)

// Parse the JSON string back to an object
const exampleParsed = JSON.parse(exampleJSON) as WorkflowActionOneOf
console.log(exampleParsed)
```

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


