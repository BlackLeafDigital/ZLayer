
# ContainerResourceLimits

Resource limits for a container

## Properties

Name | Type
------------ | -------------
`cpu` | number
`memory` | string

## Example

```typescript
import type { ContainerResourceLimits } from '@zlayer/client'

// TODO: Update the object below with actual values
const example = {
  "cpu": null,
  "memory": null,
} satisfies ContainerResourceLimits

console.log(example)

// Convert the instance to a JSON string
const exampleJSON: string = JSON.stringify(example)
console.log(exampleJSON)

// Parse the JSON string back to an object
const exampleParsed = JSON.parse(exampleJSON) as ContainerResourceLimits
console.log(exampleParsed)
```

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


