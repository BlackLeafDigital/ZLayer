
# NetworkAttachmentInfo

Per-network attachment entry on [`ContainerInfo::networks`].  Populated from the runtime\'s inspect response — mirrors the subset of bollard\'s `EndpointSettings` that API clients need to correlate a container with its `container_networks` entries.

## Properties

Name | Type
------------ | -------------
`aliases` | Array&lt;string&gt;
`ipv4` | string
`network` | string

## Example

```typescript
import type { NetworkAttachmentInfo } from '@zlayer/api-client'

// TODO: Update the object below with actual values
const example = {
  "aliases": null,
  "ipv4": null,
  "network": null,
} satisfies NetworkAttachmentInfo

console.log(example)

// Convert the instance to a JSON string
const exampleJSON: string = JSON.stringify(example)
console.log(exampleJSON)

// Parse the JSON string back to an object
const exampleParsed = JSON.parse(exampleJSON) as NetworkAttachmentInfo
console.log(exampleParsed)
```

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


