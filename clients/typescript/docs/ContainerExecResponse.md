
# ContainerExecResponse

Exec response with command output

## Properties

Name | Type
------------ | -------------
`exitCode` | number
`stderr` | string
`stdout` | string

## Example

```typescript
import type { ContainerExecResponse } from '@zlayer/client'

// TODO: Update the object below with actual values
const example = {
  "exitCode": null,
  "stderr": null,
  "stdout": null,
} satisfies ContainerExecResponse

console.log(example)

// Convert the instance to a JSON string
const exampleJSON: string = JSON.stringify(example)
console.log(exampleJSON)

// Parse the JSON string back to an object
const exampleParsed = JSON.parse(exampleJSON) as ContainerExecResponse
console.log(exampleParsed)
```

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


