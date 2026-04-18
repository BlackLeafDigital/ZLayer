
# HealthResponse

Health check response

## Properties

Name | Type
------------ | -------------
`runtimeName` | string
`status` | string
`uptimeSecs` | number
`version` | string

## Example

```typescript
import type { HealthResponse } from '@zlayer/client'

// TODO: Update the object below with actual values
const example = {
  "runtimeName": null,
  "status": null,
  "uptimeSecs": null,
  "version": null,
} satisfies HealthResponse

console.log(example)

// Convert the instance to a JSON string
const exampleJSON: string = JSON.stringify(example)
console.log(exampleJSON)

// Parse the JSON string back to an object
const exampleParsed = JSON.parse(exampleJSON) as HealthResponse
console.log(exampleParsed)
```

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


