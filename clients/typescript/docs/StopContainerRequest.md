
# StopContainerRequest

Request body for stopping a container. Matches the Docker-compat `POST /containers/{id}/stop` shape.

## Properties

Name | Type
------------ | -------------
`timeout` | number

## Example

```typescript
import type { StopContainerRequest } from '@zlayer/client'

// TODO: Update the object below with actual values
const example = {
  "timeout": null,
} satisfies StopContainerRequest

console.log(example)

// Convert the instance to a JSON string
const exampleJSON: string = JSON.stringify(example)
console.log(exampleJSON)

// Parse the JSON string back to an object
const exampleParsed = JSON.parse(exampleJSON) as StopContainerRequest
console.log(exampleParsed)
```

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


