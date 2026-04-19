
# ContainerInfo

Container information returned by the API

## Properties

Name | Type
------------ | -------------
`createdAt` | string
`id` | string
`image` | string
`labels` | { [key: string]: string; }
`name` | string
`pid` | number
`state` | string

## Example

```typescript
import type { ContainerInfo } from '@zlayer/client'

// TODO: Update the object below with actual values
const example = {
  "createdAt": null,
  "id": null,
  "image": null,
  "labels": null,
  "name": null,
  "pid": null,
  "state": null,
} satisfies ContainerInfo

console.log(example)

// Convert the instance to a JSON string
const exampleJSON: string = JSON.stringify(example)
console.log(exampleJSON)

// Parse the JSON string back to an object
const exampleParsed = JSON.parse(exampleJSON) as ContainerInfo
console.log(exampleParsed)
```

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


