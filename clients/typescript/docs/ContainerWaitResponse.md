
# ContainerWaitResponse

Wait response with container exit code

## Properties

Name | Type
------------ | -------------
`exitCode` | number
`id` | string

## Example

```typescript
import type { ContainerWaitResponse } from '@zlayer/client'

// TODO: Update the object below with actual values
const example = {
  "exitCode": null,
  "id": null,
} satisfies ContainerWaitResponse

console.log(example)

// Convert the instance to a JSON string
const exampleJSON: string = JSON.stringify(example)
console.log(exampleJSON)

// Parse the JSON string back to an object
const exampleParsed = JSON.parse(exampleJSON) as ContainerWaitResponse
console.log(exampleParsed)
```

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


