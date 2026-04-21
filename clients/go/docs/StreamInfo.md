# StreamInfo

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**BackendCount** | **int32** | Number of backends. | 
**Backends** | [**[]StreamBackendInfo**](StreamBackendInfo.md) | Backend addresses. | 
**Port** | **int32** | Listen port on this node. | 
**Protocol** | **string** | Transport protocol (&#x60;tcp&#x60; or &#x60;udp&#x60;). | 
**Service** | **string** | Service name. | 

## Methods

### NewStreamInfo

`func NewStreamInfo(backendCount int32, backends []StreamBackendInfo, port int32, protocol string, service string, ) *StreamInfo`

NewStreamInfo instantiates a new StreamInfo object
This constructor will assign default values to properties that have it defined,
and makes sure properties required by API are set, but the set of arguments
will change when the set of required properties is changed

### NewStreamInfoWithDefaults

`func NewStreamInfoWithDefaults() *StreamInfo`

NewStreamInfoWithDefaults instantiates a new StreamInfo object
This constructor will only assign default values to properties that have it defined,
but it doesn't guarantee that properties required by API are set

### GetBackendCount

`func (o *StreamInfo) GetBackendCount() int32`

GetBackendCount returns the BackendCount field if non-nil, zero value otherwise.

### GetBackendCountOk

`func (o *StreamInfo) GetBackendCountOk() (*int32, bool)`

GetBackendCountOk returns a tuple with the BackendCount field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetBackendCount

`func (o *StreamInfo) SetBackendCount(v int32)`

SetBackendCount sets BackendCount field to given value.


### GetBackends

`func (o *StreamInfo) GetBackends() []StreamBackendInfo`

GetBackends returns the Backends field if non-nil, zero value otherwise.

### GetBackendsOk

`func (o *StreamInfo) GetBackendsOk() (*[]StreamBackendInfo, bool)`

GetBackendsOk returns a tuple with the Backends field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetBackends

`func (o *StreamInfo) SetBackends(v []StreamBackendInfo)`

SetBackends sets Backends field to given value.


### GetPort

`func (o *StreamInfo) GetPort() int32`

GetPort returns the Port field if non-nil, zero value otherwise.

### GetPortOk

`func (o *StreamInfo) GetPortOk() (*int32, bool)`

GetPortOk returns a tuple with the Port field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetPort

`func (o *StreamInfo) SetPort(v int32)`

SetPort sets Port field to given value.


### GetProtocol

`func (o *StreamInfo) GetProtocol() string`

GetProtocol returns the Protocol field if non-nil, zero value otherwise.

### GetProtocolOk

`func (o *StreamInfo) GetProtocolOk() (*string, bool)`

GetProtocolOk returns a tuple with the Protocol field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetProtocol

`func (o *StreamInfo) SetProtocol(v string)`

SetProtocol sets Protocol field to given value.


### GetService

`func (o *StreamInfo) GetService() string`

GetService returns the Service field if non-nil, zero value otherwise.

### GetServiceOk

`func (o *StreamInfo) GetServiceOk() (*string, bool)`

GetServiceOk returns a tuple with the Service field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetService

`func (o *StreamInfo) SetService(v string)`

SetService sets Service field to given value.



[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


