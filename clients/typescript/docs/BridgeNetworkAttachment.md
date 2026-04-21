
# BridgeNetworkAttachment

A container attached to a [`BridgeNetwork`].

## Properties

Name | Type
------------ | -------------
`aliases` | Array&lt;string&gt;
`containerId` | string
`containerName` | string
`ipv4` | string

## Example

```typescript
import type { BridgeNetworkAttachment } from '@zlayer/api-client'

// TODO: Update the object below with actual values
const example = {
  "aliases": null,
  "containerId": null,
  "containerName": null,
  "ipv4": null,
} satisfies BridgeNetworkAttachment

console.log(example)

// Convert the instance to a JSON string
const exampleJSON: string = JSON.stringify(example)
console.log(exampleJSON)

// Parse the JSON string back to an object
const exampleParsed = JSON.parse(exampleJSON) as BridgeNetworkAttachment
console.log(exampleParsed)
```

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


