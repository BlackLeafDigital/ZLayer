
# KillContainerRequest

Request body for killing (sending a signal to) a container. Matches the Docker-compat `POST /containers/{id}/kill` shape.

## Properties

Name | Type
------------ | -------------
`signal` | string

## Example

```typescript
import type { KillContainerRequest } from '@zlayer/client'

// TODO: Update the object below with actual values
const example = {
  "signal": null,
} satisfies KillContainerRequest

console.log(example)

// Convert the instance to a JSON string
const exampleJSON: string = JSON.stringify(example)
console.log(exampleJSON)

// Parse the JSON string back to an object
const exampleParsed = JSON.parse(exampleJSON) as KillContainerRequest
console.log(exampleParsed)
```

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


