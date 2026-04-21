
# ContainerInfo

Container information returned by the API

## Properties

Name | Type
------------ | -------------
`createdAt` | string
`exitCode` | number
`health` | [ContainerHealthInfo](ContainerHealthInfo.md)
`id` | string
`image` | string
`ipv4` | string
`labels` | { [key: string]: string; }
`name` | string
`networks` | [Array&lt;NetworkAttachmentInfo&gt;](NetworkAttachmentInfo.md)
`pid` | number
`ports` | [Array&lt;PortMapping&gt;](PortMapping.md)
`state` | string

## Example

```typescript
import type { ContainerInfo } from '@zlayer/api-client'

// TODO: Update the object below with actual values
const example = {
  "createdAt": null,
  "exitCode": null,
  "health": null,
  "id": null,
  "image": null,
  "ipv4": null,
  "labels": null,
  "name": null,
  "networks": null,
  "pid": null,
  "ports": null,
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


