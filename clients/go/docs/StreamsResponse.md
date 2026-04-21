# StreamsResponse

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**Streams** | [**[]StreamInfo**](StreamInfo.md) | Stream details. | 
**TcpCount** | **int32** | TCP stream count. | 
**Total** | **int32** | Total number of stream proxies. | 
**UdpCount** | **int32** | UDP stream count. | 

## Methods

### NewStreamsResponse

`func NewStreamsResponse(streams []StreamInfo, tcpCount int32, total int32, udpCount int32, ) *StreamsResponse`

NewStreamsResponse instantiates a new StreamsResponse object
This constructor will assign default values to properties that have it defined,
and makes sure properties required by API are set, but the set of arguments
will change when the set of required properties is changed

### NewStreamsResponseWithDefaults

`func NewStreamsResponseWithDefaults() *StreamsResponse`

NewStreamsResponseWithDefaults instantiates a new StreamsResponse object
This constructor will only assign default values to properties that have it defined,
but it doesn't guarantee that properties required by API are set

### GetStreams

`func (o *StreamsResponse) GetStreams() []StreamInfo`

GetStreams returns the Streams field if non-nil, zero value otherwise.

### GetStreamsOk

`func (o *StreamsResponse) GetStreamsOk() (*[]StreamInfo, bool)`

GetStreamsOk returns a tuple with the Streams field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetStreams

`func (o *StreamsResponse) SetStreams(v []StreamInfo)`

SetStreams sets Streams field to given value.


### GetTcpCount

`func (o *StreamsResponse) GetTcpCount() int32`

GetTcpCount returns the TcpCount field if non-nil, zero value otherwise.

### GetTcpCountOk

`func (o *StreamsResponse) GetTcpCountOk() (*int32, bool)`

GetTcpCountOk returns a tuple with the TcpCount field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetTcpCount

`func (o *StreamsResponse) SetTcpCount(v int32)`

SetTcpCount sets TcpCount field to given value.


### GetTotal

`func (o *StreamsResponse) GetTotal() int32`

GetTotal returns the Total field if non-nil, zero value otherwise.

### GetTotalOk

`func (o *StreamsResponse) GetTotalOk() (*int32, bool)`

GetTotalOk returns a tuple with the Total field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetTotal

`func (o *StreamsResponse) SetTotal(v int32)`

SetTotal sets Total field to given value.


### GetUdpCount

`func (o *StreamsResponse) GetUdpCount() int32`

GetUdpCount returns the UdpCount field if non-nil, zero value otherwise.

### GetUdpCountOk

`func (o *StreamsResponse) GetUdpCountOk() (*int32, bool)`

GetUdpCountOk returns a tuple with the UdpCount field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetUdpCount

`func (o *StreamsResponse) SetUdpCount(v int32)`

SetUdpCount sets UdpCount field to given value.



[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


