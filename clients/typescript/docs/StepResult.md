
# StepResult

Result of executing a single step in a workflow run.

## Properties

Name | Type
------------ | -------------
`output` | string
`status` | string
`stepName` | string

## Example

```typescript
import type { StepResult } from '@zlayer/client'

// TODO: Update the object below with actual values
const example = {
  "output": null,
  "status": null,
  "stepName": null,
} satisfies StepResult

console.log(example)

// Convert the instance to a JSON string
const exampleJSON: string = JSON.stringify(example)
console.log(exampleJSON)

// Parse the JSON string back to an object
const exampleParsed = JSON.parse(exampleJSON) as StepResult
console.log(exampleParsed)
```

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


