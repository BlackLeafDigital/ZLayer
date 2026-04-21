
# NetworkAttachmentRequest

A request to attach a freshly-created container to a user-defined bridge or overlay network, mirroring the wire-shape used by `POST /api/v1/container-networks/{id_or_name}/connect`.  Included on [`CreateContainerRequest::networks`] so callers can wire up every attachment in a single call instead of issuing a separate connect request per network after container create.

## Properties

Name | Type
------------ | -------------
`aliases` | Array&lt;string&gt;
`ipv4Address` | string
`network` | string

## Example

```typescript
import type { NetworkAttachmentRequest } from '@zlayer/api-client'

// TODO: Update the object below with actual values
const example = {
  "aliases": null,
  "ipv4Address": null,
  "network": null,
} satisfies NetworkAttachmentRequest

console.log(example)

// Convert the instance to a JSON string
const exampleJSON: string = JSON.stringify(example)
console.log(exampleJSON)

// Parse the JSON string back to an object
const exampleParsed = JSON.parse(exampleJSON) as NetworkAttachmentRequest
console.log(exampleParsed)
```

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


