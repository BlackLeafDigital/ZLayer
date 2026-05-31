# NatCandidateDto

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**Address** | **string** | Address (host:port) | 
**Kind** | **string** | &#x60;Host&#x60; / &#x60;ServerReflexive&#x60; / &#x60;Relay&#x60; | 
**Priority** | **int32** | Priority (higher &#x3D; preferred) | 
**Transport** | **string** | Transport (e.g. \&quot;udp\&quot;) | 

## Methods

### NewNatCandidateDto

`func NewNatCandidateDto(address string, kind string, priority int32, transport string, ) *NatCandidateDto`

NewNatCandidateDto instantiates a new NatCandidateDto object
This constructor will assign default values to properties that have it defined,
and makes sure properties required by API are set, but the set of arguments
will change when the set of required properties is changed

### NewNatCandidateDtoWithDefaults

`func NewNatCandidateDtoWithDefaults() *NatCandidateDto`

NewNatCandidateDtoWithDefaults instantiates a new NatCandidateDto object
This constructor will only assign default values to properties that have it defined,
but it doesn't guarantee that properties required by API are set

### GetAddress

`func (o *NatCandidateDto) GetAddress() string`

GetAddress returns the Address field if non-nil, zero value otherwise.

### GetAddressOk

`func (o *NatCandidateDto) GetAddressOk() (*string, bool)`

GetAddressOk returns a tuple with the Address field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetAddress

`func (o *NatCandidateDto) SetAddress(v string)`

SetAddress sets Address field to given value.


### GetKind

`func (o *NatCandidateDto) GetKind() string`

GetKind returns the Kind field if non-nil, zero value otherwise.

### GetKindOk

`func (o *NatCandidateDto) GetKindOk() (*string, bool)`

GetKindOk returns a tuple with the Kind field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetKind

`func (o *NatCandidateDto) SetKind(v string)`

SetKind sets Kind field to given value.


### GetPriority

`func (o *NatCandidateDto) GetPriority() int32`

GetPriority returns the Priority field if non-nil, zero value otherwise.

### GetPriorityOk

`func (o *NatCandidateDto) GetPriorityOk() (*int32, bool)`

GetPriorityOk returns a tuple with the Priority field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetPriority

`func (o *NatCandidateDto) SetPriority(v int32)`

SetPriority sets Priority field to given value.


### GetTransport

`func (o *NatCandidateDto) GetTransport() string`

GetTransport returns the Transport field if non-nil, zero value otherwise.

### GetTransportOk

`func (o *NatCandidateDto) GetTransportOk() (*string, bool)`

GetTransportOk returns a tuple with the Transport field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetTransport

`func (o *NatCandidateDto) SetTransport(v string)`

SetTransport sets Transport field to given value.



[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


