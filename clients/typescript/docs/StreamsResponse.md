
# StreamsResponse

Response for `GET /api/v1/proxy/streams`.

## Properties

Name | Type
------------ | -------------
`streams` | [Array&lt;StreamInfo&gt;](StreamInfo.md)
`tcpCount` | number
`total` | number
`udpCount` | number

## Example

```typescript
import type { StreamsResponse } from '@zlayer/client'

// TODO: Update the object below with actual values
const example = {
  "streams": null,
  "tcpCount": null,
  "total": null,
  "udpCount": null,
} satisfies StreamsResponse

console.log(example)

// Convert the instance to a JSON string
const exampleJSON: string = JSON.stringify(example)
console.log(exampleJSON)

// Parse the JSON string back to an object
const exampleParsed = JSON.parse(exampleJSON) as StreamsResponse
console.log(exampleParsed)
```

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


